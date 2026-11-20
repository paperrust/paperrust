// upstream: dateparser 1.4.3 __init__.py (parse), date.py (sanitize_date, sanitize_spaces,
//           DateDataParser.get_date_data, _get_applicable_locales, _DateLocaleParser),
//           date_parser.py (DateParser.parse), conf.py (check_settings: DATE_ORDER)
//
// Entry point `dateparser_parse` == `dateparser.parse(date_string, settings={...},
// locales=...)` for the settings paperless passes. Parsers tried per locale, in
// dateparser's default order: timestamp (omitted: the date regex never yields a
// 10-digit string), relative-time, custom-formats (none given), absolute-time.

use std::sync::LazyLock;

use chrono::{DateTime, FixedOffset, Utc};
use regex::Regex;

use super::dp_fresh;
use super::dp_locale::{Locale, load_locales};
use super::dp_parser::{ParseSettings, PreferDayOfMonth, parse_absolute};
use super::dp_tz::{apply_timezone, fixed_offset, localize, pop_tz_offset, resolve_tz};

/// Settings for one `dateparser.parse` call.
#[derive(Debug, Clone)]
pub struct DpSettings {
    pub date_order: String,
    pub prefer_day_of_month: PreferDayOfMonth,
    /// `TIMEZONE` setting (pytz name or abbreviation)
    pub timezone: String,
    /// `RETURN_AS_TIMEZONE_AWARE`: true (paperless parse) / "default" (parse_ignore_dates)
    pub return_aware: bool,
    /// stands in for `datetime.now()` inside dateparser
    pub now: DateTime<Utc>,
}

const DATE_ORDERS: [&str; 6] = ["DMY", "DYM", "MDY", "MYD", "YDM", "YMD"];

static RE_SKIP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)\t|\n|\r|\u{00bb}|,\s\u{0432}\b|\u{200e}|\u{b7}|\u{200f}|\u{064e}|\u{064f}").unwrap());
static RE_RUSSIAN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)([\W\d])\u{0433}\.").unwrap());
static RE_CROATIAN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(\d+)\.\s?(\d+)\.\s?(\d+)\.( u)?").unwrap());
static RE_SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static RE_TRIM_SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s+(\S.*?)\s+\z").unwrap());
static RE_PERIOD: LazyLock<fancy_regex::Regex> = LazyLock::new(|| fancy_regex::Regex::new(r"(?<=[^0-9\s])\.").unwrap());
static RE_ON: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^.*?on:\s+(.*)").unwrap());
static RE_TRIM_COLONS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\S.*?):*\z").unwrap());
static RE_APOS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("\u{2019}|\u{02bc}|\u{02bb}|\u{055a}|\u{a78c}|\u{2032}|\u{2035}|\u{02b9}|\u{ff07}").unwrap());

fn sanitize_spaces(s: &str) -> String {
    let s = s.replace('\u{a0}', " ");
    let s = RE_SPACES.replace_all(&s, " ").into_owned();
    RE_TRIM_SPACES.replace_all(&s, "$1").into_owned()
}

/// `dateparser.date.sanitize_date`
pub(crate) fn sanitize_date(s: &str) -> String {
    let s = RE_SKIP.replace_all(s, " ");
    let s = RE_RUSSIAN.replace_all(&s, "$1 ");
    let s = RE_CROATIAN.replace_all(&s, "$1.$2.$3 ");
    let s = sanitize_spaces(&s);
    let s = RE_PERIOD.replace_all(&s, "").into_owned();
    let s = RE_ON.replace_all(&s, "$1");
    let s = RE_TRIM_COLONS.replace_all(&s, "$1");
    let s = RE_APOS.replace_all(&s, "'");
    crate::pyre::py_strip(&s).to_string()
}

/// Error raised by `dateparser.parse` itself (paperless logs it and returns None).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateparserError(pub String);

fn parse_with_locale(locale: &Locale, s: &str, st: &DpSettings) -> Result<Option<DateTime<FixedOffset>>, DateparserError> {
    let translated = locale.translate(s).map_err(DateparserError)?;

    // relative-time
    if let Some((d, _period)) = dp_fresh::parse(&translated, &st.timezone, st.now) {
        return Ok(Some(d));
    }

    // absolute-time (date_parser.parse with DATE_ORDER explicitly set)
    if translated.trim().is_empty() {
        return Ok(None);
    }
    let stripped = dp_fresh::strip_braces(&translated);
    let (ds, ptz) = pop_tz_offset(&stripped);
    if ds.is_empty() {
        return Ok(None);
    }
    let ps = ParseSettings {
        date_order: st.date_order.clone(),
        prefer_day: st.prefer_day_of_month,
        now_utc: st.now.naive_utc(),
        today_year: chrono::Datelike::year(&st.now.with_timezone(&chrono::Local).date_naive()),
    };
    let Ok((naive, _period)) = parse_absolute(&ds, &ps) else { return Ok(None) };
    let tz_lower = st.timezone.to_lowercase();
    let aware = match ptz {
        Some((off, _)) => {
            let fo = fixed_offset(off);
            let d = chrono::TimeZone::from_local_datetime(&fo, &naive).single().ok_or_else(|| DateparserError("tz".into()))?;
            if tz_lower.contains("local") { d } else { apply_timezone(d, &st.timezone) }
        }
        None => {
            if tz_lower.contains("local") {
                let l = chrono::TimeZone::from_local_datetime(&chrono::Local, &naive)
                    .earliest()
                    .ok_or_else(|| DateparserError("local tz".into()))?;
                l.with_timezone(l.offset())
            } else {
                let tz = resolve_tz(&st.timezone)
                    .ok_or_else(|| DateparserError(format!("UnknownTimeZoneError('{}')", st.timezone)))?;
                localize(naive, &tz)
            }
        }
    };
    let _ = st.return_aware;
    Ok(Some(aware))
}

/// `dateparser.parse(date_string, settings=..., locales=locales)`.
/// `locales` empty -> all languages in dateparser's default order.
pub fn dateparser_parse(
    date_string: &str,
    st: &DpSettings,
    locales: &[String],
) -> Result<Option<DateTime<FixedOffset>>, DateparserError> {
    if !DATE_ORDERS.contains(&st.date_order.as_str()) {
        return Err(DateparserError(format!("\"DATE_ORDER\" setting has invalid value: {}", st.date_order)));
    }
    let locs = load_locales(locales).map_err(DateparserError)?;
    let s = sanitize_date(date_string);
    if s.is_empty() {
        // Dictionary.split("") -> no tokens -> are_tokens_valid() is False for every
        // locale; short-circuit instead of compiling all locale data
        return Ok(None);
    }

    // the variant of the string without timezone is only used for applicability
    let (stripped, tz) = pop_tz_offset(&s);
    let alt = if tz.is_some() && stripped != s { Some(stripped) } else { None };

    for locale in &locs {
        let mut tries = 0;
        if locale.is_applicable(&s).map_err(DateparserError)? {
            tries += 1;
        }
        if let Some(a) = &alt {
            if locale.is_applicable(a).map_err(DateparserError)? {
                tries += 1;
            }
        }
        // a locale applicable to both variants is yielded twice; parsing is deterministic,
        // so a second attempt cannot change the result
        if tries > 0 {
            if let Some(d) = parse_with_locale(locale, &s, st)? {
                return Ok(Some(d));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, TimeZone};

    fn st(order: &str, tz: &str) -> DpSettings {
        DpSettings {
            date_order: order.into(),
            prefer_day_of_month: PreferDayOfMonth::First,
            timezone: tz.into(),
            return_aware: true,
            now: Utc.with_ymd_and_hms(2026, 11, 7, 12, 0, 0).unwrap(),
        }
    }

    fn ymd(s: &str, order: &str, locales: &[&str]) -> Option<(i32, u32, u32)> {
        let l: Vec<String> = locales.iter().map(|x| x.to_string()).collect();
        dateparser_parse(s, &st(order, "UTC"), &l).ok().flatten().map(|d| (d.year(), d.month(), d.day()))
    }

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_date("  15.\tJan.  2024 "), "15. Jan 2024");
        assert_eq!(sanitize_date("abc:"), "abc");
    }

    #[test]
    fn parse_examples() {
        assert_eq!(ymd("15 Jan 2024", "DMY", &["en"]), Some((2024, 1, 15)));
        assert_eq!(ymd("January 15, 2024", "DMY", &["en"]), Some((2024, 1, 15)));
        assert_eq!(ymd("15. März 2024", "DMY", &["de"]), Some((2024, 3, 15)));
        assert_eq!(ymd("15. März 2024", "DMY", &["en"]), None);
        assert_eq!(ymd("Monday 2024", "DMY", &["en"]), Some((2024, 11, 7)));
        assert_eq!(ymd("not a date", "DMY", &["en"]), None);
        assert_eq!(ymd("", "DMY", &["en"]), None);
        assert_eq!(ymd("abc123xyz", "DMY", &["en"]), None);
        assert_eq!(ymd("1985-05-01", "YMD", &[]), Some((1985, 5, 1)));
        assert!(dateparser_parse("1/2/2020", &st("XYZ", "UTC"), &[]).is_err());
        assert!(dateparser_parse("1/2/2020", &st("DMY", "UTC"), &["xx".into()]).is_err());
    }
}
