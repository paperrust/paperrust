//! Cron expressions with the semantics of `celery.schedules.crontab`.
//!
//! upstream: celery/schedules.py@5.6.3 (`crontab_parser`, `crontab._expand_cronspec`,
//! `crontab.remaining_delta`, `crontab.is_due`), celery/utils/time.py (`weekday`, `yearmonth`,
//! `remaining`, `ffwd`), src/paperless/settings/custom.py (`parse_beat_schedule`:
//! `minute, hour, day_month, month, day_week = value.split(" ")`).
//!
//! Semantics (checked against the Celery source, golden values from Celery 5.6.3 in the tests):
//! * Fields `minute` (0–59), `hour` (0–23), `day_of_week` (0–6, 0 = Sunday), `day_of_month`
//!   (1–31), `month_of_year` (1–12).
//! * Parts: `*`, `*/n`, `a-b`, `a-b/n` (range with wrap-around, e.g. `50-10`), number, list with `,`.
//!   The regexes are anchored like `re.match` **at the start only** (`1-5-9` = `1-5`,
//!   `*/5/2` = `*/5`).
//! * Names: month name first (`jan`…`dec`), then weekday (`sun`…`sat`), each the first three
//!   letters case-insensitively – **in every field** (Celery quirk: `mar` in the
//!   weekday field = 3 = Wednesday).
//! * Days: **AND** combination of day-of-month and weekday (unlike Vixie cron).
//! * Next run: smallest wall-clock time in `CELERY_TIMEZONE` (= `PAPERLESS_TIME_ZONE`) with
//!   second 0 whose minute lies after the minute of `last_run_at` and matches all fields.
//!   A non-existent local time (DST gap) is resolved like Python's `zoneinfo` with `fold=0`
//!   (offset **before** the switch, 02:10 → 03:10 CEST); an ambiguous time → the first.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use regex::Regex;

/// Parse error (texts like Celery: `ParseException` or `ValueError`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CronError {
    /// `celery.schedules.ParseException`
    #[error("ParseException: {0}")]
    Parse(String),
    /// `ValueError`
    #[error("ValueError: {0}")]
    Value(String),
}

const DAYNAMES: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
const MONTHNAMES: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

static RANGE_STEPS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\w+?)-(\w+)/(\w+)?").expect("static regex"));
static RANGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\w+?)-(\w+)").expect("static regex"));
static STAR_STEPS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\*/(\w+)?").expect("static regex"));

/// `crontab_parser(max_, min_)`.
#[derive(Debug, Clone, Copy)]
pub struct CrontabParser {
    max: u32,
    min: u32,
}

/// Python `int(s)` for a `\w+` token (ASCII digits only, optional `_` between digits).
fn python_int(s: &str) -> Option<i64> {
    let t = s.trim();
    if t.is_empty() || t.starts_with('_') || t.ends_with('_') || t.contains("__") {
        return None;
    }
    let digits: String = t.chars().filter(|c| *c != '_').collect();
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse::<i64>().ok()
}

impl CrontabParser {
    /// Parser for values `min..min+max-1`.
    pub const fn new(max: u32, min: u32) -> Self {
        Self { max, min }
    }

    /// `parse(spec)` → set of values.
    pub fn parse(&self, spec: &str) -> Result<BTreeSet<u32>, CronError> {
        let mut acc = BTreeSet::new();
        for part in spec.split(',') {
            if part.is_empty() {
                return Err(CronError::Parse("empty part".into()));
            }
            acc.extend(self.parse_part(part)?);
        }
        Ok(acc)
    }

    fn parse_part(&self, part: &str) -> Result<Vec<u32>, CronError> {
        if let Some(c) = RANGE_STEPS.captures(part) {
            let fr = c.get(1).map_or("", |m| m.as_str());
            let to = c.get(2).map_or("", |m| m.as_str());
            let step = c.get(3).map(|m| m.as_str()).filter(|s| !s.is_empty());
            let Some(step) = step else {
                return Err(CronError::Parse("empty filter".into()));
            };
            let r = self.expand_range(fr, Some(to))?;
            return Self::slice_step(r, step);
        }
        if let Some(c) = RANGE.captures(part) {
            let fr = c.get(1).map_or("", |m| m.as_str());
            let to = c.get(2).map_or("", |m| m.as_str());
            return self.expand_range(fr, Some(to));
        }
        if let Some(c) = STAR_STEPS.captures(part) {
            let Some(step) = c.get(1).map(|m| m.as_str()).filter(|s| !s.is_empty()) else {
                return Err(CronError::Parse("empty filter".into()));
            };
            return Self::slice_step(self.expand_star(), step);
        }
        if part == "*" {
            return Ok(self.expand_star());
        }
        self.expand_range(part, None)
    }

    /// Python `list[::int(step)]`.
    fn slice_step(v: Vec<u32>, step: &str) -> Result<Vec<u32>, CronError> {
        let Some(n) = python_int(step) else {
            return Err(CronError::Value(format!("invalid literal for int() with base 10: '{step}'")));
        };
        if n == 0 {
            return Err(CronError::Value("slice step cannot be zero".into()));
        }
        if n < 0 {
            // `\w` does not allow '-'; the case does not occur.
            return Err(CronError::Value("negative step".into()));
        }
        let n = usize::try_from(n).unwrap_or(usize::MAX);
        Ok(v.into_iter().step_by(n).collect())
    }

    fn expand_star(&self) -> Vec<u32> {
        (self.min..self.max + self.min).collect()
    }

    fn expand_range(&self, fr: &str, to: Option<&str>) -> Result<Vec<u32>, CronError> {
        let fr = self.expand_number(fr)?;
        if let Some(to) = to {
            let to = self.expand_number(to)?;
            if to < fr {
                let mut v: Vec<u32> = (fr..self.min + self.max).collect();
                v.extend(self.min..=to);
                return Ok(v);
            }
            return Ok((fr..=to).collect());
        }
        Ok(vec![fr])
    }

    fn expand_number(&self, s: &str) -> Result<u32, CronError> {
        if s.starts_with('-') {
            return Err(CronError::Parse("negative numbers not supported".into()));
        }
        let i: i64 = match python_int(s) {
            Some(i) => i,
            None => {
                let abbr: String = s.chars().take(3).collect::<String>().to_lowercase();
                if let Some(m) = MONTHNAMES.iter().position(|n| *n == abbr) {
                    m as i64 + 1
                } else if let Some(d) = DAYNAMES.iter().position(|n| *n == abbr) {
                    d as i64
                } else {
                    return Err(CronError::Value(format!("Invalid weekday literal {}.", py_repr(s))));
                }
            }
        };
        let max_val = i64::from(self.min) + i64::from(self.max) - 1;
        if i > max_val {
            return Err(CronError::Value(format!("Invalid end range: {i} > {max_val}.")));
        }
        if i < i64::from(self.min) {
            return Err(CronError::Value(format!("Invalid beginning range: {i} < {}.", self.min)));
        }
        Ok(i as u32)
    }
}

fn py_repr(s: &str) -> String {
    if s.contains('\'') && !s.contains('"') { format!("\"{s}\"") } else { format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")) }
}

/// `celery.schedules.crontab` (string fields only, as in paperless).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crontab {
    pub minute: BTreeSet<u32>,
    pub hour: BTreeSet<u32>,
    pub day_of_week: BTreeSet<u32>,
    pub day_of_month: BTreeSet<u32>,
    pub month_of_year: BTreeSet<u32>,
    /// Original texts (for display/comparison), order like the `crontab(...)` arguments.
    pub orig: [String; 5],
}

impl Crontab {
    /// `crontab(minute, hour, day_of_week, day_of_month, month_of_year)`.
    pub fn new(minute: &str, hour: &str, day_of_week: &str, day_of_month: &str, month_of_year: &str) -> Result<Self, CronError> {
        Ok(Self {
            minute: CrontabParser::new(60, 0).parse(minute)?,
            hour: CrontabParser::new(24, 0).parse(hour)?,
            day_of_week: CrontabParser::new(7, 0).parse(day_of_week)?,
            day_of_month: CrontabParser::new(31, 1).parse(day_of_month)?,
            month_of_year: CrontabParser::new(12, 1).parse(month_of_year)?,
            orig: [minute.into(), hour.into(), day_of_week.into(), day_of_month.into(), month_of_year.into()],
        })
    }

    /// paperless format `"m h dom mon dow"`, exactly one space as separator
    /// (`value.split(" ")` with unpacking into five variables).
    pub fn from_paperless(value: &str) -> Result<Self, CronError> {
        let parts: Vec<&str> = value.split(' ').collect();
        if parts.len() != 5 {
            let msg = if parts.len() > 5 {
                "too many values to unpack (expected 5)".to_string()
            } else {
                format!("not enough values to unpack (expected 5, got {})", parts.len())
            };
            return Err(CronError::Value(msg));
        }
        Self::new(parts[0], parts[1], parts[4], parts[2], parts[3])
    }

    fn matches_date(&self, d: NaiveDate) -> bool {
        self.month_of_year.contains(&d.month())
            && self.day_of_month.contains(&d.day())
            && self.day_of_week.contains(&d.weekday().num_days_from_sunday())
    }

    /// Next matching wall-clock time (naive, local time) strictly after the minute of `after`.
    pub fn next_local_after(&self, after: NaiveDateTime) -> Option<NaiveDateTime> {
        let start = after.with_second(0)?.with_nanosecond(0)? + Duration::minutes(1);
        let mut date = start.date();
        let mut first_day = true;
        // At most ~8 years (29 February with a weekday can take 28 years → 2000 months
        // like Celery's roll_over limit, counted here in days).
        for _ in 0..(366 * 60) {
            if self.matches_date(date) {
                let (h0, m0) = if first_day { (start.hour(), start.minute()) } else { (0, 0) };
                for &h in self.hour.range(h0..) {
                    let mstart = if h == h0 { m0 } else { 0 };
                    if let Some(&m) = self.minute.range(mstart..).next() {
                        return Some(date.and_time(NaiveTime::from_hms_opt(h, m, 0)?));
                    }
                }
            }
            date = date.succ_opt()?;
            first_day = false;
        }
        None
    }

    /// Next run after `last_run_at` as a UTC instant (local time in `tz`).
    pub fn next_after(&self, last_run_at: DateTime<Utc>, tz: Tz) -> Option<DateTime<Utc>> {
        let local = last_run_at.with_timezone(&tz).naive_local();
        let next = self.next_local_after(local)?;
        Some(resolve_local(tz, next))
    }

    /// `crontab.is_due(last_run_at)` → `(due, seconds until the next run)`.
    /// If the entry is due, the wait is recomputed from `now` (like Celery).
    pub fn is_due(&self, last_run_at: DateTime<Utc>, now: DateTime<Utc>, tz: Tz) -> (bool, f64) {
        let Some(next) = self.next_after(last_run_at, tz) else {
            return (false, f64::INFINITY);
        };
        if next <= now {
            let rem = self.next_after(now, tz).map_or(f64::INFINITY, |n| secs(n - now).max(0.0));
            (true, rem)
        } else {
            (false, secs(next - now).max(0.0))
        }
    }
}

fn secs(d: Duration) -> f64 {
    d.num_microseconds().map_or(d.num_seconds() as f64, |us| us as f64 / 1_000_000.0)
}

/// Naive local time → UTC like Python `datetime(..., tzinfo=ZoneInfo(tz))` with `fold=0`.
pub fn resolve_local(tz: Tz, naive: NaiveDateTime) -> DateTime<Utc> {
    match tz.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) => dt.with_timezone(&Utc),
        chrono::LocalResult::Ambiguous(a, b) => a.with_timezone(&Utc).min(b.with_timezone(&Utc)),
        chrono::LocalResult::None => {
            // Gap: offset before the switch (the instant one day earlier is unambiguous enough).
            let before = naive - Duration::days(1);
            let offset = match tz.from_local_datetime(&before) {
                chrono::LocalResult::Single(dt) | chrono::LocalResult::Ambiguous(dt, _) => {
                    chrono::Offset::fix(dt.offset()).local_minus_utc()
                }
                chrono::LocalResult::None => 0,
            };
            Utc.from_utc_datetime(&(naive - Duration::seconds(i64::from(offset))))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(spec: &str, max: u32, min: u32) -> Vec<u32> {
        CrontabParser::new(max, min).parse(spec).unwrap().into_iter().collect()
    }

    #[test]
    fn parser_matches_celery() {
        // Golden from celery 5.6.3 `crontab_parser(max, min).parse(spec)`.
        assert_eq!(set("*/15", 60, 0), vec![0, 15, 30, 45]);
        assert_eq!(set("1-5/2", 60, 0), vec![1, 3, 5]);
        let mut wrap: Vec<u32> = (0..=10).collect();
        wrap.extend(50..60);
        assert_eq!(set("50-10", 60, 0), wrap);
        assert_eq!(set("mon", 7, 0), vec![1]);
        assert_eq!(set("mar", 7, 0), vec![3]);
        assert_eq!(set("sun-sat", 7, 0), vec![0, 1, 2, 3, 4, 5, 6]);
        assert_eq!(set("jan,mar", 12, 1), vec![1, 3]);
        assert_eq!(set("*/3", 31, 1), vec![1, 4, 7, 10, 13, 16, 19, 22, 25, 28, 31]);
        assert_eq!(set("1-5-9", 60, 0), vec![1, 2, 3, 4, 5]);
        assert_eq!(set("*/5/2", 60, 0), (0..60).step_by(5).collect::<Vec<_>>());
        assert_eq!(set("Sunday", 7, 0), vec![0]);
        assert_eq!(set("2-12/2", 12, 1), vec![2, 4, 6, 8, 10, 12]);
    }

    #[test]
    fn parser_errors_match_celery() {
        let err = |spec: &str, max: u32, min: u32| CrontabParser::new(max, min).parse(spec).unwrap_err().to_string();
        assert_eq!(err("7", 7, 0), "ValueError: Invalid end range: 7 > 6.");
        assert_eq!(err("*/", 60, 0), "ParseException: empty filter");
        assert_eq!(err("", 60, 0), "ParseException: empty part");
        assert_eq!(err("1,", 60, 0), "ParseException: empty part");
        assert_eq!(err("5/2", 60, 0), "ValueError: Invalid weekday literal '5/2'.");
        assert_eq!(err("xyz", 60, 0), "ValueError: Invalid weekday literal 'xyz'.");
        assert_eq!(err("60", 60, 0), "ValueError: Invalid end range: 60 > 59.");
        assert_eq!(err("0", 31, 1), "ValueError: Invalid beginning range: 0 < 1.");
        assert_eq!(err("1-5/", 60, 0), "ParseException: empty filter");
        assert_eq!(err("-1", 60, 0), "ParseException: negative numbers not supported");
        assert_eq!(err("*/0", 60, 0), "ValueError: slice step cannot be zero");
    }

    #[test]
    fn paperless_split_is_strict() {
        assert!(Crontab::from_paperless("*/10 * * * *").is_ok());
        assert!(Crontab::from_paperless("*/10  * * * *").is_err());
        assert!(Crontab::from_paperless("*/10 * * *").is_err());
        let c = Crontab::from_paperless("30 0 * * sun").unwrap();
        assert_eq!(c.day_of_week.iter().copied().collect::<Vec<_>>(), vec![0]);
        assert_eq!(c.orig, ["30", "0", "sun", "*", "*"].map(String::from));
    }

    /// (expression, last_run_at, expected next run UTC) – generated with celery 5.6.3:
    /// `now = last + 1s; now_utc + crontab(...).remaining_estimate(last)` with
    /// `app.conf.timezone = "Europe/Zurich"`.
    const GOLDEN: &[(&str, &str, &str)] = include!("cron_golden.in");

    #[test]
    fn next_run_matches_celery_golden() {
        let tz: Tz = "Europe/Zurich".parse().unwrap();
        let mut failures = Vec::new();
        for (expr, last, expected) in GOLDEN {
            let c = Crontab::from_paperless(expr).unwrap();
            let last = DateTime::parse_from_rfc3339(last).unwrap().with_timezone(&Utc);
            let got = c.next_after(last, tz).unwrap();
            let exp = DateTime::parse_from_rfc3339(expected).unwrap().with_timezone(&Utc);
            if got != exp {
                failures.push(format!("{expr} after {last}: got {got}, celery {exp}"));
            }
        }
        assert!(GOLDEN.len() >= 160);
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn is_due_recomputes_from_now() {
        let tz: Tz = "UTC".parse().unwrap();
        let c = Crontab::from_paperless("0 * * * *").unwrap();
        let last = DateTime::parse_from_rfc3339("2024-01-01T10:00:05Z").unwrap().with_timezone(&Utc);
        let now = DateTime::parse_from_rfc3339("2024-01-01T10:30:00Z").unwrap().with_timezone(&Utc);
        assert_eq!(c.is_due(last, now, tz), (false, 1800.0));
        let now = DateTime::parse_from_rfc3339("2024-01-01T13:15:00Z").unwrap().with_timezone(&Utc);
        // missed runs → due exactly once, next run from now
        assert_eq!(c.is_due(last, now, tz), (true, 2700.0));
    }

    #[test]
    fn dst_gap_and_overlap() {
        let tz: Tz = "Europe/Zurich".parse().unwrap();
        let gap = NaiveDate::from_ymd_opt(2024, 3, 31).unwrap().and_hms_opt(2, 10, 0).unwrap();
        assert_eq!(resolve_local(tz, gap).to_rfc3339(), "2024-03-31T01:10:00+00:00");
        let amb = NaiveDate::from_ymd_opt(2024, 10, 27).unwrap().and_hms_opt(2, 10, 0).unwrap();
        assert_eq!(resolve_local(tz, amb).to_rfc3339(), "2024-10-27T00:10:00+00:00");
    }
}
