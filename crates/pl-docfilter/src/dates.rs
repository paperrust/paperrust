//! Date/time parsers like Django forms, DRF and `dateparser` (relative expressions).
//!
//! upstream: django/forms/fields.py (DateField, DateTimeField), django/utils/dateparse.py
//! (parse_date, parse_datetime), django_filters/fields.py (IsoDateTimeField),
//! rest_framework/fields.py (DateField), src/documents/filters.py@v3.4.2 (DateQueryField).

use chrono::{
    DateTime, Datelike, Duration, FixedOffset, LocalResult, NaiveDate, NaiveDateTime,
    NaiveTime, TimeZone, Utc,
};
use chrono_tz::Tz;

use crate::pyval::py_strip;

// ---------------------------------------------------------------------------------------
// Python strptime (subset) for the Django input formats of the "en" locale.
// ---------------------------------------------------------------------------------------

/// `DATE_INPUT_FORMATS` of `django/conf/locale/en/formats.py` (LANGUAGE_CODE = "en-us").
pub const EN_DATE_INPUT_FORMATS: &[&str] = &[
    "%Y-%m-%d", "%m/%d/%Y", "%m/%d/%y", "%b %d %Y", "%b %d, %Y", "%d %b %Y", "%d %b, %Y",
    "%B %d %Y", "%B %d, %Y", "%d %B %Y", "%d %B, %Y",
];

const MONTH_ABBR: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const MONTH_FULL: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

#[derive(Default, Clone, Copy)]
struct StrpState {
    year: Option<i32>,
    month: Option<u32>,
    day: Option<u32>,
}

fn take_digits(s: &str, n: usize) -> Option<(u32, &str)> {
    if s.len() < n || !s.as_bytes()[..n].iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some((s[..n].parse().ok()?, &s[n..]))
}

/// Alternatives in the order of Python's `_strptime` regex (with backtracking).
fn month_alternatives(s: &str) -> Vec<(u32, &str)> {
    // 1[0-2]|0[1-9]|[1-9]
    let mut out = Vec::new();
    let b = s.as_bytes();
    if b.len() >= 2 && b[0] == b'1' && (b'0'..=b'2').contains(&b[1]) {
        out.push(((b[1] - b'0') as u32 + 10, &s[2..]));
    }
    if b.len() >= 2 && b[0] == b'0' && (b'1'..=b'9').contains(&b[1]) {
        out.push(((b[1] - b'0') as u32, &s[2..]));
    }
    if !b.is_empty() && (b'1'..=b'9').contains(&b[0]) {
        out.push(((b[0] - b'0') as u32, &s[1..]));
    }
    out
}

fn day_alternatives(s: &str) -> Vec<(u32, &str)> {
    // 3[01]|[12]\d|0[1-9]|[1-9]| [1-9]
    let mut out = Vec::new();
    let b = s.as_bytes();
    if b.len() >= 2 && b[0] == b'3' && (b[1] == b'0' || b[1] == b'1') {
        out.push((30 + (b[1] - b'0') as u32, &s[2..]));
    }
    if b.len() >= 2 && (b[0] == b'1' || b[0] == b'2') && b[1].is_ascii_digit() {
        out.push(((b[0] - b'0') as u32 * 10 + (b[1] - b'0') as u32, &s[2..]));
    }
    if b.len() >= 2 && b[0] == b'0' && (b'1'..=b'9').contains(&b[1]) {
        out.push(((b[1] - b'0') as u32, &s[2..]));
    }
    if !b.is_empty() && (b'1'..=b'9').contains(&b[0]) {
        out.push(((b[0] - b'0') as u32, &s[1..]));
    }
    if b.len() >= 2 && b[0] == b' ' && (b'1'..=b'9').contains(&b[1]) {
        out.push(((b[1] - b'0') as u32, &s[2..]));
    }
    out
}

fn name_alternatives<'a>(s: &'a str, names: &[&str; 12]) -> Vec<(u32, &'a str)> {
    let mut out = Vec::new();
    for (i, n) in names.iter().enumerate() {
        if s.len() >= n.len() && s.is_char_boundary(n.len()) && s[..n.len()].eq_ignore_ascii_case(n)
        {
            out.push((i as u32 + 1, &s[n.len()..]));
        }
    }
    out
}

fn strp_match(fmt: &str, s: &str, st: StrpState) -> Option<StrpState> {
    if fmt.is_empty() {
        return if s.is_empty() { Some(st) } else { None };
    }
    let fb = fmt.as_bytes();
    if fb[0] == b'%' && fb.len() >= 2 {
        let rest_fmt = &fmt[2..];
        let alts: Vec<(u32, &str)> = match fb[1] {
            b'Y' => take_digits(s, 4).into_iter().collect(),
            b'y' => take_digits(s, 2)
                .map(|(y, r)| (if y <= 68 { y + 2000 } else { y + 1900 }, r))
                .into_iter()
                .collect(),
            b'm' => month_alternatives(s),
            b'd' => day_alternatives(s),
            b'b' => name_alternatives(s, &MONTH_ABBR),
            b'B' => name_alternatives(s, &MONTH_FULL),
            _ => return None,
        };
        for (v, rest) in alts {
            let mut next = st;
            match fb[1] {
                b'Y' | b'y' => next.year = Some(v as i32),
                b'm' | b'b' | b'B' => next.month = Some(v),
                _ => next.day = Some(v),
            }
            if let Some(r) = strp_match(rest_fmt, rest, next) {
                return Some(r);
            }
        }
        return None;
    }
    if fb[0] == b' ' {
        // Python replaces whitespace in the format with \s+
        let trimmed = s.trim_start_matches(char::is_whitespace);
        if trimmed.len() == s.len() {
            return None;
        }
        return strp_match(&fmt[1..], trimmed, st);
    }
    let c = fmt.chars().next()?;
    if s.starts_with(c) {
        return strp_match(&fmt[c.len_utf8()..], &s[c.len_utf8()..], st);
    }
    None
}

/// `datetime.datetime.strptime(value, fmt).date()` for the directives used above.
pub fn py_strptime_date(value: &str, fmt: &str) -> Option<NaiveDate> {
    let st = strp_match(fmt, value, StrpState::default())?;
    NaiveDate::from_ymd_opt(st.year.unwrap_or(1900), st.month.unwrap_or(1), st.day.unwrap_or(1))
}

/// `django.forms.DateField.to_python` with the en input formats.
/// `Ok(None)`: empty value; `Err(())`: "Enter a valid date."
#[allow(clippy::result_unit_err)]
pub fn django_form_date(value: &str) -> Result<Option<NaiveDate>, ()> {
    if value.is_empty() {
        return Ok(None);
    }
    let v = py_strip(value);
    for f in EN_DATE_INPUT_FORMATS {
        if let Some(d) = py_strptime_date(v, f) {
            return Ok(Some(d));
        }
    }
    Err(())
}

// ---------------------------------------------------------------------------------------
// Python date/datetime.fromisoformat (3.11+) and Django's parse_date/parse_datetime
// ---------------------------------------------------------------------------------------

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn iso_week_date(year: i32, week: u32, day: u32) -> Option<NaiveDate> {
    NaiveDate::from_isoywd_opt(year, week, chrono::Weekday::try_from((day as u8).checked_sub(1)?).ok()?)
}

/// Python `date.fromisoformat` (date part). Returns (date, length of the date part).
fn py_iso_date_prefix(s: &str) -> Option<(NaiveDate, usize)> {
    let b = s.as_bytes();
    if b.len() < 7 || !s.get(..4).is_some_and(all_digits) {
        return None;
    }
    let year: i32 = s[..4].parse().ok()?;
    if b[4] == b'-' {
        if b[5] == b'W' {
            // YYYY-Www[-D]
            let week: u32 = if all_digits(s.get(6..8)?) { s[6..8].parse().ok()? } else { return None };
            if b.len() >= 10 && b[8] == b'-' && b[9].is_ascii_digit() {
                let day = (b[9] - b'0') as u32;
                return Some((iso_week_date(year, week, day)?, 10));
            }
            return Some((iso_week_date(year, week, 1)?, 8));
        }
        let month: u32 = if all_digits(s.get(5..7)?) { s[5..7].parse().ok()? } else { return None };
        if b.get(7) != Some(&b'-') {
            return None;
        }
        let day: u32 = if all_digits(s.get(8..10)?) { s[8..10].parse().ok()? } else { return None };
        return Some((NaiveDate::from_ymd_opt(year, month, day)?, 10));
    }
    if b[4] == b'W' {
        let week: u32 = if all_digits(s.get(5..7)?) { s[5..7].parse().ok()? } else { return None };
        if b.len() >= 8 && b[7].is_ascii_digit() {
            return Some((iso_week_date(year, week, (b[7] - b'0') as u32)?, 8));
        }
        return Some((iso_week_date(year, week, 1)?, 7));
    }
    // YYYYMMDD
    let month: u32 = if all_digits(s.get(4..6)?) { s[4..6].parse().ok()? } else { return None };
    let day: u32 = if all_digits(s.get(6..8)?) { s[6..8].parse().ok()? } else { return None };
    Some((NaiveDate::from_ymd_opt(year, month, day)?, 8))
}

/// Python `date.fromisoformat(s)`.
pub fn py_date_fromisoformat(s: &str) -> Option<NaiveDate> {
    let (d, len) = py_iso_date_prefix(s)?;
    if len == s.len() { Some(d) } else { None }
}

/// Django's `date_re`: `(?P<year>\d{4})-(?P<month>\d{1,2})-(?P<day>\d{1,2})$`
/// Result: None = no match, Some(None) = match but invalid date (ValueError).
fn django_date_re(s: &str) -> Option<Option<NaiveDate>> {
    let s = s.strip_suffix('\n').unwrap_or(s);
    let parts: Vec<&str> = s.splitn(3, '-').collect();
    if parts.len() != 3 {
        return None;
    }
    let (y, m, d) = (parts[0], parts[1], parts[2]);
    if y.len() != 4 || !all_digits(y) || !(1..=2).contains(&m.len()) || !all_digits(m)
        || !(1..=2).contains(&d.len()) || !all_digits(d)
    {
        return None;
    }
    let (y, m, d): (i32, u32, u32) = (y.parse().ok()?, m.parse().ok()?, d.parse().ok()?);
    Some(NaiveDate::from_ymd_opt(y, m, d))
}

/// DRF `DateField.to_internal_value` for strings with `input_formats = [ISO_8601]`
/// (Django's `parse_date`). `None`: format error.
pub fn drf_parse_date(s: &str) -> Option<NaiveDate> {
    if let Some(d) = py_date_fromisoformat(s) {
        return Some(d);
    }
    django_date_re(s).flatten()
}

/// Result of Django's `parse_datetime`: local time plus optional fixed offset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParsedDateTime {
    pub naive: NaiveDateTime,
    pub offset: Option<FixedOffset>,
}

fn parse_frac(s: &str) -> Option<u32> {
    // any number of digits, truncated beyond 6
    if !all_digits(s) {
        return None;
    }
    let mut t: String = s.chars().take(6).collect();
    while t.len() < 6 {
        t.push('0');
    }
    t.parse().ok()
}

/// Python `time.fromisoformat`-like time part (without time zone).
fn py_iso_time(s: &str) -> Option<NaiveTime> {
    let (main, frac) = match s.find(['.', ',']) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    let (h, m, sec) = if main.contains(':') {
        let p: Vec<&str> = main.split(':').collect();
        if p.iter().any(|x| x.len() != 2 || !all_digits(x)) || p.len() > 3 {
            return None;
        }
        (
            p[0].parse().ok()?,
            p.get(1).map(|x| x.parse().unwrap_or(99)).unwrap_or(0),
            p.get(2).map(|x| x.parse().unwrap_or(99)).unwrap_or(0),
        )
    } else {
        if !all_digits(main) || ![2, 4, 6].contains(&main.len()) {
            return None;
        }
        (
            main[..2].parse().ok()?,
            main.get(2..4).map(|x| x.parse().unwrap_or(99)).unwrap_or(0),
            main.get(4..6).map(|x| x.parse().unwrap_or(99)).unwrap_or(0),
        )
    };
    let micro = match frac {
        Some(f) => {
            // Fractions only allowed after seconds
            if (main.contains(':') && main.len() != 8) || (!main.contains(':') && main.len() != 6) {
                return None;
            }
            parse_frac(f)?
        }
        None => 0,
    };
    NaiveTime::from_hms_micro_opt(h, m, sec, micro)
}

fn py_iso_tz(s: &str) -> Option<FixedOffset> {
    if s == "Z" {
        return FixedOffset::east_opt(0);
    }
    let sign = match s.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let body = &s[1..];
    let t = py_iso_time(body)?;
    let secs = t.signed_duration_since(NaiveTime::MIN).num_seconds() as i32;
    FixedOffset::east_opt(sign * secs)
}

/// Python 3.11+ `datetime.fromisoformat(s)` (common forms).
pub fn py_datetime_fromisoformat(s: &str) -> Option<ParsedDateTime> {
    let (date, len) = py_iso_date_prefix(s)?;
    if len == s.len() {
        return Some(ParsedDateTime {
            naive: date.and_time(NaiveTime::MIN),
            offset: None,
        });
    }
    // any single separator character
    let rest = &s[len..];
    let sep_len = rest.chars().next()?.len_utf8();
    let rest = &rest[sep_len..];
    let tz_pos = rest.find(['+', '-', 'Z']);
    let (time_s, tz_s) = match tz_pos {
        Some(i) => (&rest[..i], Some(&rest[i..])),
        None => (rest, None),
    };
    let time = py_iso_time(time_s)?;
    let offset = match tz_s {
        Some(t) => Some(py_iso_tz(t)?),
        None => None,
    };
    Some(ParsedDateTime {
        naive: date.and_time(time),
        offset,
    })
}

/// Django's `datetime_re` fallback in `parse_datetime`. None: no match;
/// Some(None): match with invalid values (ValueError).
fn django_datetime_re(s: &str) -> Option<Option<ParsedDateTime>> {
    let s = s.strip_suffix('\n').unwrap_or(s);
    let b = s.as_bytes();
    let mut i = 0;
    let num = |i: &mut usize, min: usize, max: usize| -> Option<u32> {
        let start = *i;
        while *i < b.len() && *i - start < max && b[*i].is_ascii_digit() {
            *i += 1;
        }
        if *i - start < min {
            return None;
        }
        s[start..*i].parse().ok()
    };
    let year = num(&mut i, 4, 4)?;
    if b.get(i) != Some(&b'-') {
        return None;
    }
    i += 1;
    let month = num(&mut i, 1, 2)?;
    if b.get(i) != Some(&b'-') {
        return None;
    }
    i += 1;
    let day = num(&mut i, 1, 2)?;
    if !matches!(b.get(i), Some(b'T' | b' ')) {
        return None;
    }
    i += 1;
    let hour = num(&mut i, 1, 2)?;
    if b.get(i) != Some(&b':') {
        return None;
    }
    i += 1;
    let minute = num(&mut i, 1, 2)?;
    let mut second = 0;
    let mut micro = 0;
    if b.get(i) == Some(&b':') {
        let save = i;
        i += 1;
        match num(&mut i, 1, 2) {
            Some(sec) => {
                second = sec;
                if matches!(b.get(i), Some(b'.' | b',')) {
                    let save2 = i;
                    i += 1;
                    let start = i;
                    while i < b.len() && i - start < 12 && b[i].is_ascii_digit() {
                        i += 1;
                    }
                    if i == start {
                        i = save2;
                    } else {
                        let digits = &s[start..i.min(start + 6)];
                        micro = parse_frac(digits)?;
                    }
                }
            }
            None => i = save,
        }
    }
    while i < b.len() && (b[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    let mut offset = None;
    if i < b.len() {
        let tz = &s[i..];
        if tz == "Z" {
            offset = FixedOffset::east_opt(0);
        } else {
            if !tz.is_ascii() {
                return None;
            }
            let tb = tz.as_bytes();
            let ok_len = matches!(tb.len(), 3 | 5 | 6);
            if !ok_len || !matches!(tb[0], b'+' | b'-') || !all_digits(&tz[1..3]) {
                return None;
            }
            let mins = match tb.len() {
                3 => 0,
                5 if all_digits(&tz[3..5]) => tz[3..5].parse::<i32>().ok()?,
                6 if tb[3] == b':' && all_digits(&tz[4..6]) => tz[4..6].parse::<i32>().ok()?,
                _ => return None,
            };
            let mut off = 60 * tz[1..3].parse::<i32>().ok()? + mins;
            if tb[0] == b'-' {
                off = -off;
            }
            match FixedOffset::east_opt(off * 60) {
                Some(o) => offset = Some(o),
                None => return Some(None),
            }
        }
    }
    let date = NaiveDate::from_ymd_opt(year as i32, month, day);
    let time = NaiveTime::from_hms_micro_opt(hour, minute, second, micro);
    Some(match (date, time) {
        (Some(d), Some(t)) => Some(ParsedDateTime {
            naive: d.and_time(t),
            offset,
        }),
        _ => None,
    })
}

/// Django's `parse_datetime`. `Err(())` corresponds to ValueError, `Ok(None)` to no match.
#[allow(clippy::result_unit_err)]
pub fn django_parse_datetime(s: &str) -> Result<Option<ParsedDateTime>, ()> {
    if let Some(p) = py_datetime_fromisoformat(s) {
        return Ok(Some(p));
    }
    match django_datetime_re(s) {
        None => Ok(None),
        Some(Some(p)) => Ok(Some(p)),
        Some(None) => Err(()),
    }
}

/// Error when parsing an `IsoDateTimeFilter` value.
#[derive(Debug, Clone, PartialEq)]
pub enum DateTimeInputError {
    /// "Enter a valid date/time."
    Invalid,
    /// "… couldn’t be interpreted in time zone …"
    Ambiguous { naive: NaiveDateTime },
}

/// `django_filters.fields.IsoDateTimeField.clean` (incl. `from_current_timezone`).
pub fn iso_datetime_field(value: &str, tz: Tz) -> Result<Option<DateTime<Utc>>, DateTimeInputError> {
    if value.is_empty() {
        return Ok(None);
    }
    let v = py_strip(value);
    let parsed = match django_parse_datetime(v) {
        Ok(Some(p)) => p,
        _ => return Err(DateTimeInputError::Invalid),
    };
    match parsed.offset {
        Some(off) => match off.from_local_datetime(&parsed.naive) {
            LocalResult::Single(dt) => Ok(Some(dt.with_timezone(&Utc))),
            _ => Err(DateTimeInputError::Invalid),
        },
        None => match tz.from_local_datetime(&parsed.naive) {
            LocalResult::Single(dt) => Ok(Some(dt.with_timezone(&Utc))),
            _ => Err(DateTimeInputError::Ambiguous {
                naive: parsed.naive,
            }),
        },
    }
}

/// Python `str(datetime)` for naive times.
pub fn py_naive_datetime_str(dt: &NaiveDateTime) -> String {
    let base = dt.format("%Y-%m-%d %H:%M:%S").to_string();
    let micro = dt.and_utc().timestamp_subsec_micros();
    if micro != 0 {
        format!("{base}.{micro:06}")
    } else {
        base
    }
}

/// Local time → instant like `timezone.make_aware` with zoneinfo (fold=0).
pub fn local_to_utc_fold0(naive: NaiveDateTime, tz: Tz) -> DateTime<Utc> {
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) => dt.with_timezone(&Utc),
        LocalResult::Ambiguous(a, _) => a.with_timezone(&Utc),
        LocalResult::None => {
            // Gap: fold=0 takes the offset before the transition.
            let probe = naive.and_utc() - Duration::hours(30);
            let off = tz.offset_from_utc_datetime(&probe.naive_utc());
            let secs = chrono::Offset::fix(&off).local_minus_utc() as i64;
            (naive - Duration::seconds(secs)).and_utc()
        }
    }
}

/// First instant whose local date (in `tz`) is >= `date`.
pub fn start_of_local_day(date: NaiveDate, tz: Tz) -> DateTime<Utc> {
    local_to_utc_fold0(date.and_time(NaiveTime::MIN), tz)
}

// ---------------------------------------------------------------------------------------
// Relative date expressions (dateparser, parser "relative-time")
// ---------------------------------------------------------------------------------------

/// Which languages the relative parser may use: `locales=[*DATE_PARSER_LANGUAGES, "en"]`, without
/// the setting all (here: all from `relative_dp_data.rs`). Regional variants (`de-AT`) count as their
/// language; dateparser knows Norwegian as "nb", "no"/"nn" count as an alias here.
fn language_enabled(langs: Option<&[String]>, code: &str) -> bool {
    match langs {
        None | Some([]) => true,
        Some(list) => {
            code == "en"
                || list.iter().any(|l| {
                    let l = l.to_ascii_lowercase();
                    let l = if code == "nb" && (l == "no" || l == "nn" || l.starts_with("no-")) { "nb".to_string() } else { l };
                    l == code || l.starts_with(&format!("{code}-"))
                })
        }
    }
}

/// Relative date expression like `dateparser.parse(value, locales=…, settings={"PARSERS": ["relative-time"],
/// "RELATIVE_BASE": now})` (upstream `DateQueryField.to_internal_value`), relative to `now` (local time).
/// Re-implementation with the dateparser language data, see [`crate::relative_dp`].
pub fn parse_relative_date(value: &str, now_local: NaiveDateTime, langs: Option<&[String]>) -> Option<NaiveDate> {
    crate::relative_dp::parse(value, now_local, |code| language_enabled(langs, code))
}

/// Local "now" time in `tz`.
pub fn now_local(now: DateTime<Utc>, tz: Tz) -> NaiveDateTime {
    now.with_timezone(&tz).naive_local()
}

/// `datetime.date(year, 1, 1)` bounds (Django `year_lookup_bounds_for_date_field`).
pub fn year_bounds(year: i64, iso: bool) -> Option<(NaiveDate, NaiveDate)> {
    let y = i32::try_from(year).ok()?;
    if !(1..=9999).contains(&y) {
        return None;
    }
    if iso {
        let first = NaiveDate::from_isoywd_opt(y, 1, chrono::Weekday::Mon)?;
        if y == 9999 {
            return None;
        }
        let next = NaiveDate::from_isoywd_opt(y + 1, 1, chrono::Weekday::Mon)?;
        Some((first, next.pred_opt()?))
    } else {
        Some((NaiveDate::from_ymd_opt(y, 1, 1)?, NaiveDate::from_ymd_opt(y, 12, 31)?))
    }
}

/// Bounds for `year` lookups on DateTimeField in the current time zone:
/// [start of year, start of the following year).
pub fn year_bounds_datetime(year: i64, tz: Tz) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let (first, _) = year_bounds(year, false)?;
    let start = start_of_local_day(first, tz);
    let end = match NaiveDate::from_ymd_opt(first.year() + 1, 1, 1) {
        Some(n) => start_of_local_day(n, tz),
        None => DateTime::<Utc>::MAX_UTC,
    };
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, dd: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, dd).unwrap()
    }

    #[test]
    fn form_date_formats() {
        assert_eq!(django_form_date("2024-01-05"), Ok(Some(d(2024, 1, 5))));
        assert_eq!(django_form_date("2024-1-5"), Ok(Some(d(2024, 1, 5))));
        assert_eq!(django_form_date(" 10/25/2006 "), Ok(Some(d(2006, 10, 25))));
        assert_eq!(django_form_date("10/25/06"), Ok(Some(d(2006, 10, 25))));
        assert_eq!(django_form_date("10/25/69"), Ok(Some(d(1969, 10, 25))));
        assert_eq!(django_form_date("Oct 25 2006"), Ok(Some(d(2006, 10, 25))));
        assert_eq!(django_form_date("oct 25, 2006"), Ok(Some(d(2006, 10, 25))));
        assert_eq!(django_form_date("25 October 2006"), Ok(Some(d(2006, 10, 25))));
        assert_eq!(django_form_date("2024-02-30"), Err(()));
        assert_eq!(django_form_date("foo"), Err(()));
        assert_eq!(django_form_date(""), Ok(None));
    }

    #[test]
    fn drf_date() {
        assert_eq!(drf_parse_date("2024-08-22"), Some(d(2024, 8, 22)));
        assert_eq!(drf_parse_date("20240822"), Some(d(2024, 8, 22)));
        assert_eq!(drf_parse_date("2024-8-2"), Some(d(2024, 8, 2)));
        assert_eq!(drf_parse_date("2024-W01-1"), Some(d(2024, 1, 1)));
        assert_eq!(drf_parse_date("2024-02-30"), None);
        assert_eq!(drf_parse_date("January 2024"), None);
    }

    #[test]
    fn iso_datetime() {
        let tz: Tz = "Europe/Berlin".parse().unwrap();
        let v = iso_datetime_field("2024-01-15T10:00:00Z", tz).unwrap().unwrap();
        assert_eq!(v.to_rfc3339(), "2024-01-15T10:00:00+00:00");
        let v = iso_datetime_field("2024-01-15", tz).unwrap().unwrap();
        assert_eq!(v.to_rfc3339(), "2024-01-14T23:00:00+00:00");
        let v = iso_datetime_field("2024-1-5 3:04", tz).unwrap().unwrap();
        assert_eq!(v.to_rfc3339(), "2024-01-05T02:04:00+00:00");
        let v = iso_datetime_field("2024-01-15T10:00:00.5+01:00", tz).unwrap().unwrap();
        assert_eq!(v.to_rfc3339(), "2024-01-15T09:00:00.500+00:00");
        assert_eq!(iso_datetime_field("bogus", tz), Err(DateTimeInputError::Invalid));
        assert_eq!(iso_datetime_field("2024-13-01T00:00", tz), Err(DateTimeInputError::Invalid));
        assert!(matches!(
            iso_datetime_field("2024-03-31T02:30:00", tz),
            Err(DateTimeInputError::Ambiguous { .. })
        ));
    }

    #[test]
    fn non_ascii_input_never_panics() {
        let tz: Tz = "UTC".parse().unwrap();
        for v in ["aä€2024-01-01", "2024-01-01T10:00+1é12", "2024-01-01 10:00 +1é12", "€€€€€€€€", "2024-01-01€10:00"] {
            let _ = iso_datetime_field(v, tz);
            let _ = django_form_date(v);
            let _ = drf_parse_date(v);
        }
        assert!(iso_datetime_field("2024-01-01€10:00", tz).unwrap().is_some());
    }

    #[test]
    fn relative_dates() {
        let now = d(2024, 8, 22).and_hms_opt(23, 30, 0).unwrap();
        assert_eq!(parse_relative_date("today", now, None), Some(d(2024, 8, 22)));
        assert_eq!(parse_relative_date("in 30 days", now, None), Some(d(2024, 9, 21)));
        assert_eq!(parse_relative_date("3 days ago", now, None), Some(d(2024, 8, 19)));
        assert_eq!(parse_relative_date("1 month ago", now, None), Some(d(2024, 7, 22)));
        assert_eq!(parse_relative_date("an hour ago", now, None), Some(d(2024, 8, 22)));
        assert_eq!(parse_relative_date("in 1 hour", now, None), Some(d(2024, 8, 23)));
        assert_eq!(parse_relative_date("heute", now, Some(&["de".to_string()])), Some(d(2024, 8, 22)));
        assert_eq!(parse_relative_date("heute", now, None), Some(d(2024, 8, 22)));
        assert_eq!(parse_relative_date("vor 2 Tagen", now, None), Some(d(2024, 8, 20)));
        assert_eq!(parse_relative_date("heute", now, Some(&["en".to_string()])), None);
        assert_eq!(parse_relative_date("not a date", now, None), None);
        assert_eq!(parse_relative_date("January 2024", now, None), None);
        // W8: all ten UI languages. Expectations re-checked with dateparser 1.4.3; dateparser does not
        // recognize "tra un anno", "kahden viikon kuluttua", "în urmă cu 3 zile", "peste 2 săptămâni",
        // "daqui a 1 mês" (an earlier expectation written from memory was wrong).
        let all = None;
        assert_eq!(parse_relative_date("il y a 3 jours", now, all), Some(d(2024, 8, 19)));
        assert_eq!(parse_relative_date("dans 2 semaines", now, all), Some(d(2024, 9, 5)));
        assert_eq!(parse_relative_date("aujourd'hui", now, all), Some(d(2024, 8, 22)));
        assert_eq!(parse_relative_date("hace 1 mes", now, all), Some(d(2024, 7, 22)));
        assert_eq!(parse_relative_date("dentro de 3 días", now, all), Some(d(2024, 8, 25)));
        assert_eq!(parse_relative_date("mañana", now, all), Some(d(2024, 8, 23)));
        assert_eq!(parse_relative_date("3 giorni fa", now, all), Some(d(2024, 8, 19)));
        assert_eq!(parse_relative_date("tra un anno", now, all), None);
        assert_eq!(parse_relative_date("ieri", now, all), Some(d(2024, 8, 21)));
        assert_eq!(parse_relative_date("2 dagen geleden", now, all), Some(d(2024, 8, 20)));
        assert_eq!(parse_relative_date("over 1 week", now, all), Some(d(2024, 8, 29)));
        // W8: further dateparser languages
        assert_eq!(parse_relative_date("3 dage siden", now, all), Some(d(2024, 8, 19)));
        assert_eq!(parse_relative_date("om 2 uger", now, all), Some(d(2024, 9, 5)));
        assert_eq!(parse_relative_date("for 3 dager siden", now, all), Some(d(2024, 8, 19)));
        assert_eq!(parse_relative_date("3 päivää sitten", now, all), Some(d(2024, 8, 19)));
        assert_eq!(parse_relative_date("kahden viikon kuluttua", now, all), None);
        assert_eq!(parse_relative_date("3 дня назад", now, all), Some(d(2024, 8, 19)));
        assert_eq!(parse_relative_date("через 2 недели", now, all), Some(d(2024, 9, 5)));
        assert_eq!(parse_relative_date("Вчера", now, all), Some(d(2024, 8, 21)));
        assert_eq!(parse_relative_date("3 дні тому", now, all), Some(d(2024, 8, 19)));
        assert_eq!(parse_relative_date("3 gün önce", now, all), Some(d(2024, 8, 19)));
        assert_eq!(parse_relative_date("2 hafta sonra", now, all), Some(d(2024, 9, 5)));
        assert_eq!(parse_relative_date("3 nappal ezelőtt", now, all), Some(d(2024, 8, 19)));
        assert_eq!(parse_relative_date("2 hét múlva", now, all), Some(d(2024, 9, 5)));
        assert_eq!(parse_relative_date("acum 3 zile", now, all), Some(d(2024, 8, 19)));
        assert_eq!(parse_relative_date("în urmă cu 3 zile", now, all), None);
        assert_eq!(parse_relative_date("peste 2 săptămâni", now, all), None);
        assert_eq!(parse_relative_date("pred 3 dňami", now, all), Some(d(2024, 8, 19)));
        assert_eq!(parse_relative_date("fa 3 dies", now, all), Some(d(2024, 8, 19)));
        assert_eq!(parse_relative_date("d'aquí a 2 setmanes", now, all), Some(d(2024, 9, 5)));
        assert_eq!(parse_relative_date("πριν από 3 ημέρες", now, all), Some(d(2024, 8, 19)));
        assert_eq!(parse_relative_date("σε 2 εβδομάδες", now, all), Some(d(2024, 9, 5)));
        // Language selection: "no" is an alias for "nb"; languages not selected stay out
        assert_eq!(parse_relative_date("for 3 dager siden", now, Some(&["no".to_string()])), Some(d(2024, 8, 19)));
        assert_eq!(parse_relative_date("3 дня назад", now, Some(&["de".to_string()])), None);
        assert_eq!(parse_relative_date("3 дня назад", now, Some(&["ru".to_string()])), Some(d(2024, 8, 19)));
        // Mixed future and past word stays invalid
        assert_eq!(parse_relative_date("pred 3 dni kuluttua", now, all), None);
        assert_eq!(parse_relative_date("eergisteren", now, all), Some(d(2024, 8, 20)));
        assert_eq!(parse_relative_date("5 dni temu", now, all), Some(d(2024, 8, 17)));
        assert_eq!(parse_relative_date("za 2 tygodnie", now, all), Some(d(2024, 9, 5)));
        assert_eq!(parse_relative_date("wczoraj", now, all), Some(d(2024, 8, 21)));
        assert_eq!(parse_relative_date("há 2 dias", now, all), Some(d(2024, 8, 20)));
        assert_eq!(parse_relative_date("daqui a 1 mês", now, all), None);
        assert_eq!(parse_relative_date("amanhã", now, all), Some(d(2024, 8, 23)));
        assert_eq!(parse_relative_date("för 3 dagar sedan", now, all), Some(d(2024, 8, 19)));
        assert_eq!(parse_relative_date("om två veckor", now, all), Some(d(2024, 9, 5)));
        assert_eq!(parse_relative_date("igår", now, all), Some(d(2024, 8, 21)));
        assert_eq!(parse_relative_date("před 3 dny", now, all), Some(d(2024, 8, 19)));
        assert_eq!(parse_relative_date("za 1 měsíc", now, all), Some(d(2024, 9, 22)));
        assert_eq!(parse_relative_date("Zítra", now, all), Some(d(2024, 8, 23)));
        // Language switched off → no match
        assert_eq!(parse_relative_date("hier", now, Some(&["de".to_string()])), None);
        assert_eq!(parse_relative_date("hier", now, Some(&["fr".to_string()])), Some(d(2024, 8, 21)));
        let huge = format!("today + {} days", "9".repeat(5000));
        assert_eq!(parse_relative_date(&huge, now, None), None);
    }

    #[test]
    fn day_start_in_tz() {
        let tz: Tz = "America/Los_Angeles".parse().unwrap();
        assert_eq!(
            start_of_local_day(d(2024, 1, 1), tz).to_rfc3339(),
            "2024-01-01T08:00:00+00:00"
        );
        assert_eq!(year_bounds(2024, true), Some((d(2024, 1, 1), d(2024, 12, 29))));
        assert_eq!(year_bounds(0, false), None);
        assert_eq!(year_bounds(10000, false), None);
    }
}
