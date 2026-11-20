//! Re-implementation of the DRF field logic the mail serializers need.
//!
//! upstream: rest_framework/fields.py@3.16.1 (Field.validate_empty_values, CharField, IntegerField,
//! BooleanField, ChoiceField, DateTimeField, ListField), rest_framework/relations.py
//! (PrimaryKeyRelatedField, ManyRelatedField), django/utils/dateparse.py (parse_datetime).

use chrono::{DateTime, Datelike, Duration, FixedOffset, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde_json::{Map, Value, json};

use crate::pyval::{self, PyInt};

pub const MSG_REQUIRED: &str = "This field is required.";
pub const MSG_NULL: &str = "This field may not be null.";
pub const MSG_BLANK: &str = "This field may not be blank.";
pub const MSG_NOT_STRING: &str = "Not a valid string.";
pub const MSG_NULL_CHARS: &str = "Null characters are not allowed.";
pub const MSG_INT_INVALID: &str = "A valid integer is required.";
pub const MSG_INT_TOO_LONG: &str = "String value too large.";
pub const MSG_BOOL_INVALID: &str = "Must be a valid boolean.";
pub const MSG_DATETIME_INVALID: &str = "Datetime has wrong format. Use one of these formats instead: YYYY-MM-DDThh:mm[:ss[.uuuuuu]][+HH:MM|-HH:MM|Z].";

/// Error list of a field (`["msg", ...]`) or nested errors (`{...}`).
pub type FieldError = Value;

pub fn err(msg: impl Into<String>) -> FieldError {
    json!([msg.into()])
}

pub fn non_dict_error(v: &Value) -> Value {
    json!({ "non_field_errors": [format!(
        "Invalid data. Expected a dictionary, but got {}.",
        pyval::type_name(v)
    )] })
}

/// Raw value of a field in the request.
#[derive(Debug, Clone, Copy)]
pub enum Raw<'a> {
    Missing,
    Null,
    Val(&'a Value),
}

/// Like [`raw`] for relation fields: DRF `RelatedField.run_validation` turns `""` into `None`
/// (review 2 N26).
pub fn raw_related<'a>(data: &'a Map<String, Value>, key: &str) -> Raw<'a> {
    match data.get(key) {
        Some(Value::String(s)) if s.is_empty() => Raw::Null,
        _ => raw(data, key),
    }
}

pub fn raw<'a>(data: &'a Map<String, Value>, key: &str) -> Raw<'a> {
    match data.get(key) {
        None => Raw::Missing,
        Some(Value::Null) => Raw::Null,
        Some(v) => Raw::Val(v),
    }
}

/// Result of `validate_empty_values` + conversion.
#[derive(Debug, Clone, PartialEq)]
pub enum Got<T> {
    /// Skip the field (`SkipField`): not in the validated data.
    Skip,
    /// Value (incl. `None` for null or default).
    Set(T),
}

/// Field options for `validate_empty_values`.
#[derive(Debug, Clone, Copy)]
pub struct Opts {
    pub required: bool,
    pub allow_null: bool,
}

pub const REQUIRED: Opts = Opts { required: true, allow_null: false };
pub const OPTIONAL: Opts = Opts { required: false, allow_null: false };
pub const OPTIONAL_NULL: Opts = Opts { required: false, allow_null: true };

/// `Field.validate_empty_values`. `default` is only used when the value is missing and not partial.
/// Returns `Ok(Some(v))` if the value still has to be converted.
pub fn empty_values<'a, T>(
    r: Raw<'a>,
    opts: Opts,
    partial: bool,
    default: Option<T>,
) -> Result<Result<&'a Value, Got<Option<T>>>, FieldError> {
    match r {
        Raw::Missing => {
            if partial {
                return Ok(Err(Got::Skip));
            }
            if opts.required {
                return Err(err(MSG_REQUIRED));
            }
            match default {
                Some(d) => Ok(Err(Got::Set(Some(d)))),
                None => Ok(Err(Got::Skip)),
            }
        }
        Raw::Null => {
            if !opts.allow_null {
                return Err(err(MSG_NULL));
            }
            Ok(Err(Got::Set(None)))
        }
        Raw::Val(v) => Ok(Ok(v)),
    }
}

// ---------------------------------------------------------------------------------------------
// CharField

#[derive(Debug, Clone, Copy)]
pub struct CharOpts {
    pub allow_blank: bool,
    pub max_length: Option<usize>,
}

/// Result of `CharField.run_validation` before the validators.
#[derive(Debug, Clone, PartialEq)]
pub enum CharConv {
    /// Empty with `allow_blank` → `''`, validators do not run.
    Blank,
    /// Converted value; validators still have to run.
    Value(String),
}

/// `CharField.run_validation` up to and including `to_internal_value` (trim_whitespace=True).
pub fn char_convert(v: &Value, allow_blank: bool) -> Result<CharConv, FieldError> {
    // run_validation: '' or whitespace only → blank
    let as_str = pyval::to_str(v);
    if pyval::py_strip(&as_str).is_empty() {
        if !allow_blank {
            return Err(err(MSG_BLANK));
        }
        return Ok(CharConv::Blank);
    }
    match v {
        Value::String(s) => Ok(CharConv::Value(pyval::py_strip(s).to_string())),
        Value::Number(_) => Ok(CharConv::Value(pyval::py_strip(&as_str).to_string())),
        _ => Err(err(MSG_NOT_STRING)),
    }
}

/// `CharField.run_validation` + validators (MaxLength, ProhibitNullCharacters).
pub fn char_field(v: &Value, o: CharOpts) -> Result<String, FieldError> {
    match char_convert(v, o.allow_blank)? {
        CharConv::Blank => Ok(String::new()),
        CharConv::Value(value) => {
            let errs = char_validators(&value, o.max_length);
            if errs.is_empty() { Ok(value) } else { Err(Value::Array(errs)) }
        }
    }
}

/// MaxLength, ProhibitNullCharacters (in this order, all collected).
pub fn char_validators(value: &str, max_length: Option<usize>) -> Vec<Value> {
    let mut errs = Vec::new();
    if let Some(max) = max_length
        && value.chars().count() > max
    {
        errs.push(Value::String(format!("Ensure this field has no more than {max} characters.")));
    }
    if value.contains('\0') {
        errs.push(Value::String(MSG_NULL_CHARS.into()));
    }
    errs
}

// ---------------------------------------------------------------------------------------------
// IntegerField

/// `IntegerField.to_internal_value` + Min/Max validators (Max first).
pub fn integer_field(v: &Value, min: Option<i128>, max: Option<i128>) -> Result<i128, FieldError> {
    if let Value::String(s) = v
        && s.chars().count() > 1000
    {
        return Err(err(MSG_INT_TOO_LONG));
    }
    let s = pyval::to_str(v);
    let s = strip_decimal_zero(&s);
    let n = match v {
        Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
        _ => pyval::parse_py_int(&s),
    }
    .ok_or_else(|| err(MSG_INT_INVALID))?;
    let mut errs = Vec::new();
    if let Some(mx) = max
        && n > mx
    {
        errs.push(Value::String(format!("Ensure this value is less than or equal to {mx}.")));
    }
    if let Some(mn) = min
        && n < mn
    {
        errs.push(Value::String(format!("Ensure this value is greater than or equal to {mn}.")));
    }
    if errs.is_empty() { Ok(n) } else { Err(Value::Array(errs)) }
}

/// `re.sub(r'\.0*\s*$', '', s)`
fn strip_decimal_zero(s: &str) -> String {
    for (i, c) in s.char_indices() {
        if c == '.' {
            let rest = &s[i + 1..];
            let after_zeros = rest.trim_start_matches('0');
            if after_zeros.chars().all(pyval::is_py_space) {
                return s[..i].to_string();
            }
        }
    }
    s.to_string()
}

// ---------------------------------------------------------------------------------------------
// BooleanField

pub fn boolean_field(v: &Value) -> Result<bool, FieldError> {
    let r = match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => {
            let f = n.as_f64().unwrap_or(f64::NAN);
            if f == 1.0 {
                Some(true)
            } else if f == 0.0 {
                Some(false)
            } else {
                None
            }
        }
        Value::String(s) => match s.to_lowercase().as_str() {
            "t" | "y" | "yes" | "true" | "on" | "1" => Some(true),
            "f" | "n" | "no" | "false" | "off" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    };
    r.ok_or_else(|| err(MSG_BOOL_INVALID))
}

// ---------------------------------------------------------------------------------------------
// ChoiceField (Integer-Choices)

pub fn choice_field(v: &Value, choices: &[i16]) -> Result<i16, FieldError> {
    let s = pyval::to_str(v);
    for c in choices {
        if c.to_string() == s {
            return Ok(*c);
        }
    }
    Err(err(format!("\"{s}\" is not a valid choice.")))
}

// ---------------------------------------------------------------------------------------------
// PrimaryKeyRelatedField

/// What `queryset.get(pk=data)` would look up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PkLookup {
    /// Valid PK (fits into `integer`).
    Id(i32),
    /// Number outside the column range → DoesNotExist.
    OutOfRange,
}

/// Checks the type; error = `Incorrect type…`.
pub fn pk_lookup(v: &Value) -> Result<PkLookup, FieldError> {
    let incorrect = || {
        err(format!(
            "Incorrect type. Expected pk value, received {}.",
            pyval::type_name(v)
        ))
    };
    if v.is_boolean() {
        return Err(incorrect());
    }
    match pyval::py_int(v) {
        PyInt::Ok(i) => Ok(i32::try_from(i).map(PkLookup::Id).unwrap_or(PkLookup::OutOfRange)),
        PyInt::Invalid => Err(incorrect()),
    }
}

pub fn pk_does_not_exist(v: &Value) -> FieldError {
    err(format!("Invalid pk \"{}\" - object does not exist.", pyval::to_str(v)))
}

/// `ManyRelatedField.to_internal_value` (list check).
pub fn many_list(v: &Value) -> Result<Vec<Value>, FieldError> {
    match v {
        Value::Array(a) => Ok(a.clone()),
        // a dict is iterable in Python → keys
        Value::Object(o) => Ok(o.keys().map(|k| Value::String(k.clone())).collect()),
        _ => Err(err(format!(
            "Expected a list of items but got type \"{}\".",
            pyval::type_name(v)
        ))),
    }
}

// ---------------------------------------------------------------------------------------------
// DateTimeField

fn two(s: &str) -> Option<u32> {
    if s.len() == 2 && s.bytes().all(|b| b.is_ascii_digit()) { s.parse().ok() } else { None }
}

fn frac_to_micro(f: &str) -> Option<u32> {
    if f.is_empty() || !f.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut s: String = f.chars().take(6).collect();
    while s.len() < 6 {
        s.push('0');
    }
    s.parse().ok()
}

/// Time zone: Z, ±HH, ±HH:MM, ±HHMM (second offsets are not supported).
fn parse_tz(s: &str) -> Option<FixedOffset> {
    // Review 2 M4: ASCII only, otherwise the byte slices below would cut into the middle of a
    // character (panic). Django's regex `[+-]\d{2}(?::?\d{2})?` does not match then anyway → DRF 400.
    if !s.is_ascii() {
        return None;
    }
    if s == "Z" {
        return FixedOffset::east_opt(0);
    }
    let sign = match s.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let body = &s[1..];
    let (h, m) = if body.len() == 2 {
        (two(body)?, 0)
    } else if body.len() == 5 && &body[2..3] == ":" {
        (two(&body[..2])?, two(&body[3..])?)
    } else if body.len() == 4 {
        (two(&body[..2])?, two(&body[2..])?)
    } else {
        return None;
    };
    if h > 23 || m > 59 {
        return None;
    }
    FixedOffset::east_opt(sign * (h as i32 * 3600 + m as i32 * 60))
}

/// Part of Python `datetime.fromisoformat`: `YYYY-MM-DD` or `YYYYMMDD`, optionally
/// any separator character + `HH[:MM[:SS[.f]]]` / `HHMM[SS[.f]]`, optionally a time zone.
fn from_isoformat(s: &str) -> Option<(NaiveDateTime, Option<FixedOffset>)> {
    if !s.is_ascii() {
        return None;
    }
    let (date, rest) = if s.len() >= 10 && &s[4..5] == "-" && &s[7..8] == "-" {
        (&s[..10], &s[10..])
    } else if s.len() >= 8 && s[..8].bytes().all(|b| b.is_ascii_digit()) {
        (&s[..8], &s[8..])
    } else {
        return None;
    };
    let digits: String = date.chars().filter(|c| *c != '-').collect();
    if digits.len() != 8 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let d = NaiveDate::from_ymd_opt(
        digits[..4].parse().ok()?,
        digits[4..6].parse().ok()?,
        digits[6..8].parse().ok()?,
    )?;
    if rest.is_empty() {
        return Some((d.and_hms_opt(0, 0, 0)?, None));
    }
    let time = &rest[1..];
    // split off the time zone
    let tz_pos = time.find(['Z', '+', '-']);
    let (t, tz) = match tz_pos {
        Some(p) => (&time[..p], Some(parse_tz(&time[p..])?)),
        None => (time, None),
    };
    let (hms, frac) = match t.find(['.', ',']) {
        Some(p) => (&t[..p], Some(&t[p + 1..])),
        None => (t, None),
    };
    let parts: Vec<&str> = if hms.contains(':') {
        hms.split(':').collect()
    } else {
        let mut v = Vec::new();
        let mut i = 0;
        while i < hms.len() {
            v.push(hms.get(i..i + 2)?);
            i += 2;
        }
        v
    };
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let h = two(parts[0])?;
    let m = parts.get(1).map(|p| two(p)).unwrap_or(Some(0))?;
    let sec = parts.get(2).map(|p| two(p)).unwrap_or(Some(0))?;
    if frac.is_some() && parts.len() != 3 {
        return None;
    }
    let micro = match frac {
        Some(f) => frac_to_micro(f)?,
        None => 0,
    };
    let t = NaiveTime::from_hms_micro_opt(h, m, sec, micro)?;
    Some((d.and_time(t), tz))
}

/// Django `datetime_re` (fallback of `parse_datetime`).
fn django_datetime_re(s: &str) -> Option<(NaiveDateTime, Option<FixedOffset>)> {
    // Review 2 M4: the byte-wise slicing below assumes ASCII (known deviation: Python's `\d`/`\s`
    // also match Unicode digits/whitespace; such inputs → 400 here instead of Django success).
    if !s.is_ascii() {
        return None;
    }
    let b = s.as_bytes();
    let mut i = 0;
    let take_digits = |i: &mut usize, min: usize, max: usize| -> Option<u32> {
        let start = *i;
        while *i < b.len() && *i - start < max && b[*i].is_ascii_digit() {
            *i += 1;
        }
        if *i - start < min {
            return None;
        }
        s[start..*i].parse().ok()
    };
    let expect = |i: &mut usize, c: u8| -> Option<()> {
        if *i < b.len() && b[*i] == c {
            *i += 1;
            Some(())
        } else {
            None
        }
    };
    let year = take_digits(&mut i, 4, 4)?;
    expect(&mut i, b'-')?;
    let month = take_digits(&mut i, 1, 2)?;
    expect(&mut i, b'-')?;
    let day = take_digits(&mut i, 1, 2)?;
    if i < b.len() && (b[i] == b'T' || b[i] == b' ') {
        i += 1;
    } else {
        return None;
    }
    let hour = take_digits(&mut i, 1, 2)?;
    expect(&mut i, b':')?;
    let minute = take_digits(&mut i, 1, 2)?;
    let mut second = 0;
    let mut micro = 0;
    if i < b.len() && b[i] == b':' {
        i += 1;
        second = take_digits(&mut i, 1, 2)?;
        if i < b.len() && (b[i] == b'.' || b[i] == b',') {
            let save = i;
            i += 1;
            let start = i;
            while i < b.len() && i - start < 12 && b[i].is_ascii_digit() {
                i += 1;
            }
            if i == start {
                i = save;
            } else {
                micro = frac_to_micro(&s[start..i.min(start + 6)])?;
            }
        }
    }
    while i < b.len() && (b[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    let tz = if i < b.len() {
        let t = &s[i..];
        // Z | [+-]\d{2}(:?\d{2})?
        Some(parse_tz(t)?)
    } else {
        None
    };
    let d = NaiveDate::from_ymd_opt(year as i32, month, day)?;
    let t = NaiveTime::from_hms_micro_opt(hour, minute, second, micro)?;
    Some((d.and_time(t), tz))
}

/// DRF `DateTimeField.to_internal_value` with `default_timezone` = current time zone.
pub fn datetime_field(v: &Value, tz: Tz) -> Result<DateTime<Utc>, FieldError> {
    let Value::String(s) = v else {
        return Err(err(MSG_DATETIME_INVALID));
    };
    let parsed = from_isoformat(s).or_else(|| django_datetime_re(s));
    let Some((naive, offset)) = parsed else {
        return Err(err(MSG_DATETIME_INVALID));
    };
    if let Some(off) = offset {
        return off
            .from_local_datetime(&naive)
            .single()
            .map(|d| d.with_timezone(&Utc))
            .ok_or_else(|| err("Datetime value out of range."));
    }
    // make_aware(value, current tz), ambiguous → error, non-existent → offset before (fold=0)
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(d) => Ok(d.with_timezone(&Utc)),
        LocalResult::Ambiguous(_, _) => Err(err(format!(
            "Invalid datetime for the timezone \"{}\".",
            tz.name()
        ))),
        LocalResult::None => {
            let before = naive - Duration::hours(3);
            let off = tz
                .from_local_datetime(&before)
                .earliest()
                .map(|d| d.offset().fix())
                .ok_or_else(|| err(MSG_DATETIME_INVALID))?;
            off.from_local_datetime(&naive)
                .single()
                .map(|d| d.with_timezone(&Utc))
                .ok_or_else(|| err(MSG_DATETIME_INVALID))
        }
    }
}

use chrono::Offset;

/// DRF `DateTimeField.to_representation` (ISO 8601 in the current time zone).
pub fn fmt_datetime(dt: &DateTime<Utc>, tz: Tz) -> String {
    let local = dt.with_timezone(&tz);
    let off = local.offset().fix().local_minus_utc();
    let mut s = format!(
        "{:04}-{:02}-{:02}T{}",
        local.year(),
        local.month(),
        local.day(),
        local.format("%H:%M:%S")
    );
    let micros = local.timestamp_subsec_micros();
    if micros != 0 {
        s.push_str(&format!(".{micros:06}"));
    }
    if off == 0 {
        s.push('Z');
    } else {
        let sign = if off < 0 { '-' } else { '+' };
        let a = off.abs();
        s.push_str(&format!("{sign}{:02}:{:02}", a / 3600, (a % 3600) / 60));
        if a % 60 != 0 {
            s.push_str(&format!(":{:02}", a % 60));
        }
    }
    s
}

pub fn fmt_opt_datetime(dt: &Option<DateTime<Utc>>, tz: Tz) -> Value {
    match dt {
        Some(d) => Value::String(fmt_datetime(d, tz)),
        None => Value::Null,
    }
}

/// Python `datetime.now().isoformat()` (naive, server local time = TIME_ZONE).
pub fn now_isoformat(tz: Tz) -> String {
    let local = Utc::now().with_timezone(&tz);
    let micros = local.timestamp_subsec_micros();
    let mut s = local.format("%Y-%m-%dT%H:%M:%S").to_string();
    if micros != 0 {
        s.push_str(&format!(".{micros:06}"));
    }
    s
}

// ---------------------------------------------------------------------------------------------
// Error collector

/// Field errors in serializer order.
#[derive(Debug, Default)]
pub struct Errors(pub Map<String, Value>);

impl Errors {
    pub fn add(&mut self, field: &str, e: FieldError) {
        self.0.insert(field.to_string(), e);
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn into_value(self) -> Value {
        Value::Object(self.0)
    }
}

/// Convenience helper: take over the result of a field or store the error.
pub fn take<T>(errors: &mut Errors, field: &str, r: Result<Got<T>, FieldError>) -> Got<T> {
    match r {
        Ok(g) => g,
        Err(e) => {
            errors.add(field, e);
            Got::Skip
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Review 2 N26: `""` on relation fields counts as `null`.
    #[test]
    fn related_empty_string_is_null() {
        let m: Map<String, Value> = serde_json::from_str(r#"{"owner": "", "name": "", "account": 3}"#).unwrap();
        assert!(matches!(raw_related(&m, "owner"), Raw::Null));
        assert!(matches!(raw(&m, "name"), Raw::Val(_)));
        assert!(matches!(raw_related(&m, "account"), Raw::Val(_)));
        assert!(matches!(raw_related(&m, "missing"), Raw::Missing));
    }

    /// Review 2 M4: non-ASCII in time zone/date → DRF format error (400) instead of panic.
    #[test]
    fn datetime_non_ascii_is_invalid_not_panic() {
        let tz: Tz = "Europe/Zurich".parse().unwrap();
        for s in [
            "2024-01-01T10:00:00+aäb",
            "2024-01-01T10:00:00+ä",
            "2024-01-01T10:00:00+0ä",
            "2024-01-01T10:00:00+01ä0",
            "2024-01-01 10:00ü",
            "2024-01-01T10:00:00.ä",
            "２０２４-01-01T10:00",
        ] {
            assert_eq!(datetime_field(&json!(s), tz).unwrap_err(), json!([MSG_DATETIME_INVALID]), "{s}");
        }
        assert!(parse_tz("+äb").is_none());
        assert!(django_datetime_re("2024-01-01T10:00:00+aäb").is_none());
        assert!(datetime_field(&json!("2024-01-01T10:00:00+01:00"), tz).is_ok());
    }

    #[test]
    fn char_field_rules() {
        let o = CharOpts { allow_blank: false, max_length: Some(3) };
        assert_eq!(char_field(&json!("  ab "), o).unwrap(), "ab");
        assert_eq!(char_field(&json!(12), o).unwrap(), "12");
        assert_eq!(char_field(&json!("   "), o).unwrap_err(), json!([MSG_BLANK]));
        assert_eq!(char_field(&json!(true), o).unwrap_err(), json!([MSG_NOT_STRING]));
        assert_eq!(char_field(&json!([1]), o).unwrap_err(), json!([MSG_NOT_STRING]));
        assert_eq!(
            char_field(&json!("abcd\u{0}"), o).unwrap_err(),
            json!(["Ensure this field has no more than 3 characters.", MSG_NULL_CHARS])
        );
    }

    #[test]
    fn integer_field_rules() {
        assert_eq!(integer_field(&json!("12"), None, None).unwrap(), 12);
        assert_eq!(integer_field(&json!("12.000 "), None, None).unwrap(), 12);
        assert_eq!(integer_field(&json!(3.0), None, None).unwrap(), 3);
        assert_eq!(integer_field(&json!(3.5), None, None).unwrap_err(), json!([MSG_INT_INVALID]));
        assert_eq!(integer_field(&json!(true), None, None).unwrap_err(), json!([MSG_INT_INVALID]));
        assert_eq!(
            integer_field(&json!(9000000), Some(0), Some(32767)).unwrap_err(),
            json!(["Ensure this value is less than or equal to 32767."])
        );
        assert_eq!(
            integer_field(&json!(-1), Some(0), Some(32767)).unwrap_err(),
            json!(["Ensure this value is greater than or equal to 0."])
        );
    }

    #[test]
    fn bool_and_choice() {
        assert!(boolean_field(&json!("TRUE")).unwrap());
        assert!(!boolean_field(&json!(0.0)).unwrap());
        assert!(boolean_field(&json!(2)).is_err());
        assert_eq!(choice_field(&json!("2"), &[1, 2, 3]).unwrap(), 2);
        assert_eq!(
            choice_field(&json!(2.0), &[1, 2, 3]).unwrap_err(),
            json!(["\"2.0\" is not a valid choice."])
        );
        assert_eq!(
            choice_field(&json!(true), &[1, 2]).unwrap_err(),
            json!(["\"True\" is not a valid choice."])
        );
    }

    #[test]
    fn pk_rules() {
        assert_eq!(pk_lookup(&json!("5")).unwrap(), PkLookup::Id(5));
        assert_eq!(pk_lookup(&json!(5.7)).unwrap(), PkLookup::Id(5));
        assert_eq!(pk_lookup(&json!(99999999999i64)).unwrap(), PkLookup::OutOfRange);
        assert_eq!(
            pk_lookup(&json!("x")).unwrap_err(),
            json!(["Incorrect type. Expected pk value, received str."])
        );
        assert_eq!(
            pk_lookup(&json!(false)).unwrap_err(),
            json!(["Incorrect type. Expected pk value, received bool."])
        );
        assert_eq!(
            pk_does_not_exist(&json!(7)),
            json!(["Invalid pk \"7\" - object does not exist."])
        );
    }

    #[test]
    fn datetime_parse_and_format() {
        let tz: Tz = "Europe/Berlin".parse().unwrap();
        let d = datetime_field(&json!("2024-01-02T03:04:05Z"), tz).unwrap();
        assert_eq!(fmt_datetime(&d, chrono_tz::UTC), "2024-01-02T03:04:05Z");
        assert_eq!(fmt_datetime(&d, tz), "2024-01-02T04:04:05+01:00");
        let d = datetime_field(&json!("2024-07-01 10:00"), tz).unwrap();
        assert_eq!(fmt_datetime(&d, chrono_tz::UTC), "2024-07-01T08:00:00Z");
        let d = datetime_field(&json!("2024-07-01"), chrono_tz::UTC).unwrap();
        assert_eq!(fmt_datetime(&d, chrono_tz::UTC), "2024-07-01T00:00:00Z");
        let d = datetime_field(&json!("2024-7-1T1:2:3.5+0130"), chrono_tz::UTC).unwrap();
        assert_eq!(fmt_datetime(&d, chrono_tz::UTC), "2024-06-30T23:32:03.500000Z");
        assert_eq!(
            datetime_field(&json!("gestern"), tz).unwrap_err(),
            json!([MSG_DATETIME_INVALID])
        );
        assert_eq!(datetime_field(&json!(5), tz).unwrap_err(), json!([MSG_DATETIME_INVALID]));
        // ambiguous (October changeover)
        assert!(datetime_field(&json!("2024-10-27T02:30:00"), tz).is_err());
    }
}
