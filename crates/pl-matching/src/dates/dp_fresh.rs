// upstream: dateparser 1.4.3 freshness_date_parser.py (FreshnessDateDataParser)
//           + dateutil.relativedelta addition semantics
//
// paperless sets no RELATIVE_BASE, so "now" is the wall clock in settings.TIMEZONE;
// the Rust port uses the configured reference time instead (see contract).

use std::sync::LazyLock;

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, NaiveDateTime};

use super::dp_parser::time_parser;
use super::dp_tz::{apply_timezone, fixed_offset, pop_tz_offset};

const UNITS: &str = "decade|year|month|week|day|hour|minute|second";

static PATTERN: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    fancy_regex::Regex::new(&format!(r"(?is)([+-]?\s*\d++[.,]?\d*+)\s*({UNITS})\b")).unwrap()
});
static SKIP: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(&format!(r"^(?:{UNITS}|ago|in|\d+|:|[ap]m)")).unwrap());
static NON_WORD: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\W").unwrap());
static SPACES: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\s+").unwrap());
static AGO_IN: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\b(?:ago|in)\b").unwrap());
static IN_WORD: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\bin\b").unwrap());
static BRACES: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"[{}()<>\[\]]+").unwrap());

pub(crate) fn strip_braces(s: &str) -> String {
    BRACES.replace_all(s, "").into_owned()
}

fn are_all_words_units(s: &str) -> bool {
    let s = SPACES.replace_all(s.trim(), " ");
    NON_WORD.split(&s).filter(|w| !w.is_empty()).all(|w| SKIP.is_match(w))
}

fn findall(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos <= s.len() {
        let Ok(Some(c)) = PATTERN.captures_from_pos(s, pos) else { break };
        let (Some(m0), Some(n), Some(u)) = (c.get(0), c.get(1), c.get(2)) else { break };
        out.push((n.as_str().to_string(), u.as_str().to_string()));
        pos = if m0.end() > m0.start() { m0.end() } else { m0.end() + 1 };
    }
    out
}

/// relativedelta(**kwargs) applied to `now` (naive wall clock).
fn apply_delta(now: NaiveDateTime, kw: &[(String, f64)]) -> Option<NaiveDateTime> {
    let get = |k: &str| kw.iter().rev().find(|(n, _)| n == k).map(|(_, v)| *v);
    let years = get("years").unwrap_or(0.0);
    let months = get("months").unwrap_or(0.0);
    if years.fract() != 0.0 || months.fract() != 0.0 {
        return None; // relativedelta raises ValueError
    }
    let days = get("days").unwrap_or(0.0) + get("weeks").unwrap_or(0.0) * 7.0;
    let hours = get("hours").unwrap_or(0.0);
    let minutes = get("minutes").unwrap_or(0.0);
    let seconds = get("seconds").unwrap_or(0.0);

    let total_months = now.month0() as i64 + months as i64;
    let mut year = now.year() as i64 + years as i64 + total_months.div_euclid(12);
    let month = total_months.rem_euclid(12) as u32 + 1;
    if !(1..=9999).contains(&year) {
        return None;
    }
    let ny = year as i32;
    let last = NaiveDate::from_ymd_opt(if month == 12 { ny + 1 } else { ny }, if month == 12 { 1 } else { month + 1 }, 1)
        .and_then(|d| d.pred_opt())?
        .day();
    let day = now.day().min(last);
    let base = NaiveDate::from_ymd_opt(ny, month, day)?.and_time(now.time());
    let micros = days * 86_400e6 + hours * 3_600e6 + minutes * 60e6 + seconds * 1e6;
    // Python timedelta rounds half to even
    let micros = micros.round_ties_even();
    if !micros.is_finite() || micros.abs() > 1e17 {
        return None;
    }
    let out = base.checked_add_signed(Duration::microseconds(micros as i64))?;
    year = out.year() as i64;
    (1..=9999).contains(&year).then_some(out)
}

/// `freshness_date_parser.get_date_data(translated, settings)` -> (date, period)
/// `now_utc` = reference "now"; result keeps the fixed offset of `now` in `timezone`.
pub(crate) fn parse(date_string: &str, timezone: &str, now_utc: DateTime<chrono::Utc>) -> Option<(DateTime<FixedOffset>, &'static str)> {
    let s = strip_braces(date_string);
    let (s, ptz) = pop_tz_offset(&s);
    let time = {
        let t = PATTERN.replace_all(&s, "");
        let t = AGO_IN.replace_all(&t, "");
        time_parser(&t).ok()
    };

    let now: DateTime<FixedOffset> = match &ptz {
        Some((off, _)) => {
            let localized = now_utc.with_timezone(&fixed_offset(*off));
            if timezone.to_lowercase().contains("local") { localized } else { apply_timezone(localized, timezone) }
        }
        None => apply_timezone(now_utc.with_timezone(&fixed_offset(0)), timezone),
    };

    if !are_all_words_units(&s) {
        return None;
    }
    let found = findall(&s);
    if found.is_empty() {
        return None;
    }
    let mut kwargs: Vec<(String, f64)> = Vec::new();
    let mut explicit: Vec<(String, bool)> = Vec::new();
    for (num, unit) in &found {
        let key = format!("{}s", unit.to_lowercase());
        let signed = num.starts_with('+') || num.starts_with('-');
        let v: f64 = num.replace(',', ".").replace(' ', "").parse().ok()?;
        kwargs.retain(|(k, _)| *k != key);
        explicit.retain(|(k, _)| *k != key);
        kwargs.push((key.clone(), v));
        explicit.push((key, signed));
    }
    let mut period = "day";
    if !kwargs.iter().any(|(k, _)| k == "days") {
        for k in ["weeks", "months", "years", "decades"] {
            if kwargs.iter().any(|(n, _)| n == k) {
                period = match k {
                    "weeks" => "week",
                    "months" => "month",
                    _ => "year",
                };
                break;
            }
        }
    }
    let going_forward = IN_WORD.is_match(&s);
    let mut adjusted: Vec<(String, f64)> = kwargs
        .iter()
        .map(|(k, v)| {
            let signed = explicit.iter().any(|(n, s)| n == k && *s);
            (k.clone(), if signed || going_forward { *v } else { -*v })
        })
        .collect();
    if let Some(pos) = adjusted.iter().position(|(k, _)| k == "decades") {
        let dec = adjusted.remove(pos).1;
        let years = adjusted.iter().find(|(k, _)| k == "years").map_or(0.0, |(_, v)| *v);
        adjusted.retain(|(k, _)| k != "years");
        adjusted.push(("years".into(), years + 10.0 * dec));
    }
    let local = apply_delta(now.naive_local(), &adjusted)?;
    let local = match time {
        Some(t) => local.date().and_time(t),
        None => local,
    };
    let dt = now.offset().localize_naive(local)?;
    Some((dt, period))
}

trait FixedLocal {
    fn localize_naive(&self, l: NaiveDateTime) -> Option<DateTime<FixedOffset>>;
}

impl FixedLocal for FixedOffset {
    fn localize_naive(&self, l: NaiveDateTime) -> Option<DateTime<FixedOffset>> {
        use chrono::TimeZone;
        self.from_local_datetime(&l).single()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn relative() {
        let now = chrono::Utc.with_ymd_and_hms(2026, 11, 7, 12, 0, 0).unwrap();
        let (d, p) = parse("1 day ago 2024", "UTC", now).unwrap();
        assert_eq!(d.naive_local(), chrono::NaiveDate::from_ymd_opt(2026, 11, 6).unwrap().and_hms_opt(12, 0, 0).unwrap());
        assert_eq!(p, "day");
        assert!(parse("week 2024", "UTC", now).is_none());
        assert!(parse("1.5 year ago", "UTC", now).is_none());
        let (d, _) = parse("in 2 month", "UTC", now).unwrap();
        assert_eq!(d.month(), 1);
    }
}
