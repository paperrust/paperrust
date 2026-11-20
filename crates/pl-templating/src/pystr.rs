//! Python semantics for strings, numbers and paths, as far as the templates need them.
//!
//! upstream: CPython 3.12 (str.strip/str(), float.__repr__, pathlib.PurePosixPath),
//! Django 5.2 `django.utils.text.slugify`, `django.utils.dateparse`.

use chrono::{FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};

/// Python's `str.isspace()` (Unicode whitespace plus the separators U+001C..U+001F).
pub fn py_isspace(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `str.strip()` without argument.
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(py_isspace)
}

/// `str.strip(chars)`.
pub fn py_strip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_matches(|c| chars.contains(c))
}

/// Python's `repr(float)` (shortest representation, exponent from 1e16 or below 1e-4).
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
    // Rust `{:e}` yields the shortest round-trip-safe mantissa, e.g. "1.5e20"
    let sci = format!("{f:e}");
    let (mant, exp) = match sci.split_once('e') {
        Some((m, e)) => (m.to_string(), e.parse::<i32>().unwrap_or(0)),
        None => (sci.clone(), 0),
    };
    let (neg, mant) = match mant.strip_prefix('-') {
        Some(m) => (true, m.to_string()),
        None => (false, mant),
    };
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let sign = if neg { "-" } else { "" };
    if !(-4..16).contains(&exp) {
        let mut m = String::new();
        m.push_str(&digits[..1]);
        if digits.len() > 1 {
            m.push('.');
            m.push_str(&digits[1..]);
        }
        let es = if exp < 0 { format!("-{:02}", -exp) } else { format!("+{exp:02}") };
        return format!("{sign}{m}e{es}");
    }
    let n = digits.len() as i32;
    let s = if exp >= 0 {
        if n <= exp + 1 {
            format!("{}{}.0", digits, "0".repeat((exp + 1 - n) as usize))
        } else {
            let p = (exp + 1) as usize;
            format!("{}.{}", &digits[..p], &digits[p..])
        }
    } else {
        format!("0.{}{}", "0".repeat((-exp - 1) as usize), digits)
    };
    format!("{sign}{s}")
}

/// Python's `repr(str)`.
pub fn py_str_repr(s: &str) -> String {
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

/// `django.utils.text.slugify(value)` (allow_unicode=False).
pub fn django_slugify(value: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let ascii: String = value.nfkd().filter(|c| c.is_ascii()).collect();
    let lowered = ascii.to_lowercase();
    // re.sub(r"[^\w\s-]", "", value)
    let kept: String = lowered
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || py_isspace(*c) || *c == '-')
        .collect();
    // re.sub(r"[-\s]+", "-", value)
    let mut out = String::with_capacity(kept.len());
    let mut in_run = false;
    for c in kept.chars() {
        if c == '-' || py_isspace(c) {
            if !in_run {
                out.push('-');
                in_run = true;
            }
        } else {
            out.push(c);
            in_run = false;
        }
    }
    // .strip("-_")
    out.trim_matches(|c| c == '-' || c == '_').to_string()
}

// ---------------------------------------------------------------------------
// PurePosixPath (Python 3.12)
// ---------------------------------------------------------------------------

/// Parts like `PurePosixPath(p).parts` (without leading "/").
pub fn path_parts(p: &str) -> (bool, Vec<&str>) {
    let absolute = p.starts_with('/');
    let parts = p.split('/').filter(|x| !x.is_empty() && *x != ".").collect();
    (absolute, parts)
}

/// `str(PurePosixPath(p))`.
pub fn path_str(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let (absolute, parts) = path_parts(p);
    // POSIX: exactly two leading slashes are preserved
    let prefix = if p.starts_with("//") && !p.starts_with("///") {
        "//"
    } else if absolute {
        "/"
    } else {
        ""
    };
    let joined = parts.join("/");
    if joined.is_empty() && prefix.is_empty() {
        ".".into()
    } else {
        format!("{prefix}{joined}")
    }
}

/// `PurePosixPath(p).name`.
pub fn path_name(p: &str) -> &str {
    let (_, parts) = path_parts(p);
    parts.last().copied().unwrap_or("")
}

/// `PurePosixPath(p).parent` as a string (".", if none).
pub fn path_parent(p: &str) -> String {
    let s = path_str(p);
    let (absolute, parts) = path_parts(&s);
    if parts.is_empty() {
        return s;
    }
    let parent = parts[..parts.len() - 1].join("/");
    let prefix = if s.starts_with("//") {
        "//"
    } else if absolute {
        "/"
    } else {
        ""
    };
    if parent.is_empty() && prefix.is_empty() {
        ".".into()
    } else {
        format!("{prefix}{parent}")
    }
}

/// `PurePosixPath(name).suffix` (Python 3.12: no suffix for a trailing/leading dot).
pub fn name_suffix(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => &name[i..],
        _ => "",
    }
}

/// `PurePosixPath(name).stem`.
pub fn name_stem(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => &name[..i],
        _ => name,
    }
}

/// `Path(p).stem`.
pub fn path_stem(p: &str) -> &str {
    name_stem(path_name(p))
}

/// Join a path with `/` like `PurePosixPath(a) / b` (b relative).
pub fn path_join(dir: &str, name: &str) -> String {
    if dir == "." || dir.is_empty() {
        path_str(name)
    } else {
        path_str(&format!("{dir}/{name}"))
    }
}

// ---------------------------------------------------------------------------
// django.utils.dateparse / datetime.fromisoformat
// ---------------------------------------------------------------------------

/// Result of `parse_datetime`: naive or with fixed time zone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParsedDateTime {
    pub naive: NaiveDateTime,
    pub offset: Option<FixedOffset>,
}

fn take_digits(s: &str, n: usize) -> Option<(u32, &str)> {
    if s.len() < n || !s.as_bytes()[..n].iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some((s[..n].parse().ok()?, &s[n..]))
}

fn parse_iso_date(s: &str) -> Option<(NaiveDate, &str)> {
    let (y, rest) = take_digits(s, 4)?;
    if let Some(r) = rest.strip_prefix('-') {
        let (m, r) = take_digits(r, 2)?;
        let r = r.strip_prefix('-')?;
        let (d, r) = take_digits(r, 2)?;
        Some((NaiveDate::from_ymd_opt(y as i32, m, d)?, r))
    } else {
        let (m, r) = take_digits(rest, 2)?;
        let (d, r) = take_digits(r, 2)?;
        Some((NaiveDate::from_ymd_opt(y as i32, m, d)?, r))
    }
}

fn parse_offset(s: &str) -> Option<(Option<FixedOffset>, &str)> {
    if let Some(r) = s.strip_prefix('Z') {
        return Some((FixedOffset::east_opt(0), r));
    }
    let sign = match s.chars().next() {
        Some('+') => 1,
        Some('-') => -1,
        _ => return Some((None, s)),
    };
    let r = &s[1..];
    let (h, r) = take_digits(r, 2)?;
    let (m, r) = match r.strip_prefix(':') {
        Some(r2) => take_digits(r2, 2)?,
        None => take_digits(r, 2).unwrap_or((0, r)),
    };
    let (sec, r) = match r.strip_prefix(':') {
        Some(r2) => take_digits(r2, 2)?,
        None => (0, r),
    };
    let total = sign * (h as i32 * 3600 + m as i32 * 60 + sec as i32);
    Some((Some(FixedOffset::east_opt(total)?), r))
}

/// `datetime.datetime.fromisoformat` (Python 3.11+, common forms).
fn fromisoformat(s: &str) -> Option<ParsedDateTime> {
    let (date, rest) = parse_iso_date(s)?;
    if rest.is_empty() {
        return Some(ParsedDateTime { naive: date.and_time(NaiveTime::MIN), offset: None });
    }
    let mut chars = rest.chars();
    let sep = chars.next()?;
    if sep != 'T' && sep != ' ' {
        return None;
    }
    let r = &rest[sep.len_utf8()..];
    let (h, r) = take_digits(r, 2)?;
    let (mut mi, mut se, mut us) = (0, 0, 0);
    let mut r = r;
    if let Some(r2) = r.strip_prefix(':') {
        let (m, r3) = take_digits(r2, 2)?;
        mi = m;
        r = r3;
        if let Some(r4) = r.strip_prefix(':') {
            let (sv, r5) = take_digits(r4, 2)?;
            se = sv;
            r = r5;
            if let Some(r6) = r.strip_prefix('.').or_else(|| r.strip_prefix(',')) {
                let n = r6.bytes().take_while(u8::is_ascii_digit).count();
                if n == 0 {
                    return None;
                }
                let frac = &r6[..n.min(6)];
                us = frac.parse::<u32>().ok()? * 10u32.pow(6 - frac.len() as u32);
                r = &r6[n..];
            }
        }
    }
    let (offset, r) = parse_offset(r)?;
    if !r.is_empty() {
        return None;
    }
    let time = NaiveTime::from_hms_micro_opt(h, mi, se, us)?;
    Some(ParsedDateTime { naive: date.and_time(time), offset })
}

/// Django regex fallback of `parse_datetime`.
fn django_datetime_re(s: &str) -> Result<Option<ParsedDateTime>, String> {
    // (?P<year>\d{4})-(?P<month>\d{1,2})-(?P<day>\d{1,2})[T ](?P<hour>\d{1,2}):(?P<minute>\d{1,2})
    // (?::(?P<second>\d{1,2})(?:[.,](?P<microsecond>\d{1,6})\d{0,6})?)?\s*(?P<tzinfo>Z|[+-]\d{2}(?::?\d{2})?)?$
    fn digits(s: &str, min: usize, max: usize) -> Option<(&str, &str)> {
        let n = s.bytes().take_while(u8::is_ascii_digit).count();
        if n < min {
            return None;
        }
        let n = n.min(max);
        Some((&s[..n], &s[n..]))
    }
    let parse = || -> Option<(Vec<u32>, Option<FixedOffset>)> {
        let (y, r) = digits(s, 4, 4)?;
        let r = r.strip_prefix('-')?;
        let (mo, r) = digits(r, 1, 2)?;
        let r = r.strip_prefix('-')?;
        let (d, r) = digits(r, 1, 2)?;
        let r = r.strip_prefix('T').or_else(|| r.strip_prefix(' '))?;
        let (h, r) = digits(r, 1, 2)?;
        let r = r.strip_prefix(':')?;
        let (mi, mut r) = digits(r, 1, 2)?;
        let (mut se, mut us) = ("0", String::from("0"));
        if let Some(r2) = r.strip_prefix(':') {
            let (sv, r3) = digits(r2, 1, 2)?;
            se = sv;
            r = r3;
            if let Some(r4) = r.strip_prefix('.').or_else(|| r.strip_prefix(',')) {
                let (u, r5) = digits(r4, 1, 6)?;
                let (_, r6) = digits(r5, 0, 6)?;
                us = format!("{u:0<6}");
                r = r6;
            }
        }
        let r = r.trim_start_matches(py_isspace);
        let (off, r) = if r.is_empty() { (None, r) } else { parse_offset(r)? };
        if !r.is_empty() {
            return None;
        }
        let v = [y, mo, d, h, mi, se, &us].iter().map(|x| x.parse::<u32>().unwrap_or(0)).collect();
        Some((v, off))
    };
    match parse() {
        None => Ok(None),
        Some((v, off)) => {
            let dt = NaiveDate::from_ymd_opt(v[0] as i32, v[1], v[2])
                .and_then(|d| d.and_hms_micro_opt(v[3], v[4], v[5], v[6]));
            match dt {
                Some(naive) => Ok(Some(ParsedDateTime { naive, offset: off })),
                None => Err("month must be in 1..12".into()),
            }
        }
    }
}

/// `django.utils.dateparse.parse_datetime` (Django 5.2). `Err` = ValueError (well-formed but invalid).
pub fn parse_datetime(s: &str) -> Result<Option<ParsedDateTime>, String> {
    if let Some(dt) = fromisoformat(s) {
        return Ok(Some(dt));
    }
    django_datetime_re(s)
}

/// `django.utils.dateparse.parse_date`.
pub fn parse_date(s: &str) -> Result<Option<NaiveDate>, String> {
    if let Some((d, rest)) = parse_iso_date(s)
        && rest.is_empty() {
            return Ok(Some(d));
        }
    // date_re = r"(?P<year>\d{4})-(?P<month>\d{1,2})-(?P<day>\d{1,2})$"
    let mut it = s.splitn(3, '-');
    let (y, m, d) = match (it.next(), it.next(), it.next()) {
        (Some(y), Some(m), Some(d)) => (y, m, d),
        _ => return Ok(None),
    };
    let ok = y.len() == 4
        && (1..=2).contains(&m.len())
        && (1..=2).contains(&d.len())
        && [y, m, d].iter().all(|x| x.bytes().all(|b| b.is_ascii_digit()));
    if !ok {
        return Ok(None);
    }
    let (y, m, d) = (y.parse().unwrap_or(0), m.parse().unwrap_or(0), d.parse().unwrap_or(0));
    NaiveDate::from_ymd_opt(y, m, d).map(Some).ok_or_else(|| "day is out of range for month".into())
}

/// Local time of a UTC time in a time zone as a fixed offset (like `timezone.localtime`).
pub fn to_local<Tz: TimeZone>(utc: &chrono::DateTime<chrono::Utc>, tz: &Tz) -> chrono::DateTime<FixedOffset> {
    use chrono::Offset;
    let local = utc.with_timezone(tz);
    let off: FixedOffset = local.offset().fix();
    local.with_timezone(&off)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_repr() {
        assert_eq!(py_float_repr(1.0), "1.0");
        assert_eq!(py_float_repr(1.5), "1.5");
        assert_eq!(py_float_repr(1e20), "1e+20");
        assert_eq!(py_float_repr(1e-5), "1e-05");
        assert_eq!(py_float_repr(0.0001), "0.0001");
        assert_eq!(py_float_repr(123456789012345.0), "123456789012345.0");
        assert_eq!(py_float_repr(1e16), "1e+16");
        assert_eq!(py_float_repr(-2.5), "-2.5");
        assert_eq!(py_float_repr(0.1 + 0.2), "0.30000000000000004");
    }

    #[test]
    fn slugify() {
        assert_eq!(django_slugify("Some Title! With @ Special # Characters"), "some-title-with-special-characters");
        assert_eq!(django_slugify("John's @ Office / Workplace"), "johns-office-workplace");
        assert_eq!(django_slugify("Brussels @ Belgium!"), "brussels-belgium");
        assert_eq!(django_slugify("-none-"), "none");
        assert_eq!(django_slugify("Ärger über Öl"), "arger-uber-ol");
    }

    #[test]
    fn paths() {
        assert_eq!(path_str("a//b/./c/"), "a/b/c");
        assert_eq!(path_parent("a/b/c.pdf"), "a/b");
        assert_eq!(path_parent("c.pdf"), ".");
        assert_eq!(path_stem("x/some file.tar.gz"), "some file.tar");
        assert_eq!(path_stem(".bashrc"), ".bashrc");
        assert_eq!(path_stem(""), "");
        assert_eq!(name_suffix("file."), "");
    }

    #[test]
    fn dateparse() {
        let d = parse_datetime("2023-10-26").unwrap().unwrap();
        assert_eq!(d.naive.to_string(), "2023-10-26 00:00:00");
        let d = parse_datetime("2023-10-26T14:30:05+00:00").unwrap().unwrap();
        assert_eq!(d.offset.map(|o| o.local_minus_utc()), Some(0));
        assert!(parse_datetime("not a date").unwrap().is_none());
        assert!(parse_datetime("2023-1-2 3:04").unwrap().is_some());
        assert_eq!(parse_date("2023-1-2").unwrap(), NaiveDate::from_ymd_opt(2023, 1, 2));
    }
}
