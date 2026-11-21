//! Port of upstream tests:
//!   src/documents/tests/date_parsing/test_date_parsing.py@v3.4.2
//!   src/paperless/tests/settings/test_custom_parsers.py (TestParseIgnoreDates,
//!     test_parse_dateparser_languages)
//!   src/paperless/tests/test_utils.py (test_ocr_to_dateparser_languages)
//!
//! Upstream mocks `dateparser.parse` in the TestRegexDateParser cases; here the real
//! (ported) dateparser is used, which yields the same expected values.

use std::collections::HashSet;

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, TimeZone, Utc};
use pl_matching::dates::{DateParserConfig, RegexDateParser, ocr_to_dateparser_languages, parse_dateparser_languages, parse_ignore_dates};

fn utc(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<FixedOffset> {
    Utc.with_ymd_and_hms(y, m, d, h, mi, s).unwrap().fixed_offset()
}

fn base_config() -> DateParserConfig {
    DateParserConfig {
        languages: vec!["en".into()],
        timezone_str: "UTC".into(),
        ignore_dates: HashSet::new(),
        reference_time: utc(2024, 1, 15, 12, 0, 0),
        filename_date_order: Some("YMD".into()),
        content_date_order: "DMY".into(),
    }
}

fn config_with_ignore_dates() -> DateParserConfig {
    DateParserConfig {
        languages: vec!["en".into(), "de".into()],
        timezone_str: "America/New_York".into(),
        ignore_dates: [NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(), NaiveDate::from_ymd_opt(2024, 12, 25).unwrap()].into_iter().collect(),
        reference_time: utc(2024, 1, 15, 12, 0, 0),
        filename_date_order: Some("DMY".into()),
        content_date_order: "MDY".into(),
    }
}

// --- TestParseString ---------------------------------------------------------

#[test]
fn test_parse_string_valid_formats() {
    let p = RegexDateParser::new(base_config());
    for (s, order, year) in [
        ("15/01/2024", "DMY", 2024),
        ("01/15/2024", "MDY", 2024),
        ("2024/01/15", "YMD", 2024),
        ("January 15, 2024", "DMY", 2024),
        ("15 Jan 2024", "DMY", 2024),
        ("15.01.2024", "DMY", 2024),
        ("2024-01-15", "YMD", 2024),
    ] {
        let r = p.parse_string(s, order);
        assert_eq!(r.map(|d| d.year()), Some(year), "{s}");
    }
}

#[test]
fn test_parse_string_invalid_input() {
    let p = RegexDateParser::new(base_config());
    for s in ["not a date", "32/13/2024", "", "abc123xyz", "99/99/9999"] {
        assert_eq!(p.parse_string(s, "DMY"), None, "{s:?}");
    }
}

#[test]
fn test_parse_string_handles_exceptions() {
    // upstream forces dateparser.parse to raise; here a dateparser error (unknown
    // locale) must likewise be swallowed and yield None (and is logged)
    let mut cfg = base_config();
    cfg.languages = vec!["xx-invalid".into()];
    let p = RegexDateParser::new(cfg);
    assert_eq!(p.parse_string("01/01/2024", "DMY"), None);
}

// --- TestFilterDate ------------------------------------------------------------

#[test]
fn test_filter_date_validation_rules() {
    let p = RegexDateParser::new(config_with_ignore_dates());
    let cases: Vec<(Option<DateTime<FixedOffset>>, bool)> = vec![
        (Some(utc(2024, 1, 10, 0, 0, 0)), true),
        (Some(utc(2024, 1, 15, 12, 0, 0)), true),
        (Some(utc(1901, 1, 1, 0, 0, 0)), true),
        (Some(utc(2024, 1, 16, 0, 0, 0)), false),
        (Some(utc(2024, 1, 1, 0, 0, 0)), false),
        (Some(utc(2024, 1, 1, 10, 30, 0)), false),
        (Some(utc(2024, 12, 25, 15, 0, 0)), false),
        (Some(utc(1899, 12, 31, 0, 0, 0)), false),
        (Some(utc(1900, 1, 1, 0, 0, 0)), false),
        (None, false),
    ];
    for (d, keep) in cases {
        let r = p.filter_date(d);
        assert_eq!(r, if keep { d } else { None }, "{d:?}");
    }
}

#[test]
fn test_filter_date_respects_ignore_dates() {
    let p = RegexDateParser::new(config_with_ignore_dates());
    assert_eq!(p.filter_date(Some(utc(2024, 1, 1, 12, 0, 0))), None);
    assert_eq!(p.filter_date(Some(utc(2024, 12, 25, 15, 30, 0))), None);
    let allowed = utc(2024, 1, 2, 12, 0, 0);
    assert_eq!(p.filter_date(Some(allowed)), Some(allowed));
}

#[test]
fn test_filter_date_timezone_aware() {
    let p = RegexDateParser::new(base_config());
    let d = utc(2024, 1, 10, 12, 0, 0);
    assert_eq!(p.filter_date(Some(d)).map(|x| x.offset().local_minus_utc()), Some(0));
}

// --- TestRegexDateParser -------------------------------------------------------

#[test]
fn test_parse_returns_expected_dates() {
    let p = RegexDateParser::new(base_config());
    let cases = [
        ("report-2023-12-25.txt", "Event recorded on 25/12/2022.", vec![utc(2023, 12, 25, 0, 0, 0), utc(2022, 12, 25, 0, 0, 0)]),
        ("img_2023.01.02.jpg", "Taken on 01/02/2023", vec![utc(2023, 1, 2, 0, 0, 0), utc(2023, 2, 1, 0, 0, 0)]),
        ("notes.txt", "bad date 99/99/9999 and 25/12/2022", vec![utc(2022, 12, 25, 0, 0, 0)]),
    ];
    for (f, c, expected) in cases {
        let got: Vec<_> = p.parse(f, c).collect();
        assert_eq!(got, expected, "{f} / {c}");
        assert_eq!(p.parse_all(f, c), expected);
    }
}

#[test]
fn test_parse_filters_future_and_ignored_dates() {
    let cfg = DateParserConfig {
        languages: vec!["en".into()],
        timezone_str: "UTC".into(),
        ignore_dates: [NaiveDate::from_ymd_opt(2023, 12, 10).unwrap()].into_iter().collect(),
        reference_time: utc(2024, 1, 15, 12, 0, 0),
        filename_date_order: Some("YMD".into()),
        content_date_order: "DMY".into(),
    };
    let p = RegexDateParser::new(cfg);
    let content = "Ignored: 10/12/2023, Future: 01/02/2024, Keep: 05/01/2023";
    let got: Vec<_> = p.parse("whatever.txt", content).collect();
    assert_eq!(got, vec![utc(2023, 1, 5, 0, 0, 0)]);
}

#[test]
fn test_parse_handles_no_matches_and_returns_empty_list() {
    let p = RegexDateParser::new(base_config());
    assert!(p.parse("no-dates.txt", "this has no dates whatsoever").next().is_none());
}

#[test]
fn test_parse_skips_filename_when_filename_date_order_none() {
    let mut cfg = base_config();
    cfg.filename_date_order = None;
    let p = RegexDateParser::new(cfg);
    let got: Vec<_> = p.parse("report-2023-12-25.txt", "Event recorded on 25/12/2022.").collect();
    assert_eq!(got, vec![utc(2022, 12, 25, 0, 0, 0)]);
}

#[test]
fn consumer_uses_first_date_and_timezone() {
    // consumer.py: next(date_parser.parse(self.filename, text), None)
    let mut cfg = base_config();
    cfg.timezone_str = "Europe/Berlin".into();
    cfg.filename_date_order = None;
    let p = RegexDateParser::new(cfg);
    let first = p.parse("x.pdf", "Datum 03.01.2024 und 04.01.2024").next().unwrap();
    assert_eq!(first.to_rfc3339(), "2024-01-03T00:00:00+01:00");
}

// --- settings parsers -------------------------------------------------------------

#[test]
fn test_no_ignore_dates_set() {
    assert!(parse_ignore_dates("", "YMD", Utc::now()).is_empty());
}

#[test]
fn test_ignore_dates_parsed() {
    let d = |y, m, dd| NaiveDate::from_ymd_opt(y, m, dd).unwrap();
    let cases: Vec<(&str, &str, Vec<NaiveDate>)> = vec![
        ("1985-05-01", "YMD", vec![d(1985, 5, 1)]),
        ("1985-05-01,1991-12-05", "YMD", vec![d(1985, 5, 1), d(1991, 12, 5)]),
        ("2010-12-13", "YMD", vec![d(2010, 12, 13)]),
        ("11.01.10", "DMY", vec![d(2010, 1, 11)]),
        ("11.01.2001,15-06-1996", "DMY", vec![d(2001, 1, 11), d(1996, 6, 15)]),
    ];
    for (env, order, expected) in cases {
        let got = parse_ignore_dates(env, order, Utc::now());
        assert_eq!(got, expected.into_iter().collect::<HashSet<_>>(), "{env}");
    }
}

#[test]
fn test_parse_dateparser_languages() {
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("de", vec!["de"]),
        ("zh", vec!["zh"]),
        ("fr+en", vec!["fr", "en"]),
        ("en-001+fr-CA", vec!["en-001", "fr-CA"]),
        ("en-001+fr", vec!["en-001", "fr"]),
        ("en+zh-Hans-HK", vec!["en", "zh-Hans-HK", "zh"]),
        ("en+zh-Hans", vec!["en", "zh-Hans", "zh"]),
        ("en+zh-Hans+zh-Hant", vec!["en", "zh-Hans", "zh-Hant", "zh"]),
    ];
    for (input, expected) in cases {
        let mut got = parse_dateparser_languages(Some(input)).unwrap();
        got.sort();
        let mut exp: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        exp.sort();
        assert_eq!(got, exp, "{input}");
    }
    assert!(parse_dateparser_languages(Some("xx")).is_err());
}

#[test]
fn test_ocr_to_dateparser_languages() {
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("eng", vec!["en"]),
        ("fra+ita+lao", vec!["fr", "it", "lo"]),
        ("fil", vec!["fil"]),
        ("aze_cyrl+srp_latn", vec!["az-Cyrl", "sr-Latn"]),
        ("deu_frak", vec!["de"]),
        ("chi_tra+chi_sim", vec!["zh"]),
        ("eng+unsupported_language+por", vec!["en", "pt"]),
        ("unsupported1+unsupported2", vec![]),
        ("eng+eng", vec!["en"]),
        ("ita_unknownscript", vec!["it"]),
    ];
    for (input, expected) in cases {
        let mut got = ocr_to_dateparser_languages(input);
        got.sort();
        let mut exp: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        exp.sort();
        assert_eq!(got, exp, "{input}");
    }
}

#[test]
fn reference_time_timezone_comparison() {
    // date <= reference_time compares instants
    let mut cfg = base_config();
    cfg.reference_time = FixedOffset::east_opt(3600).unwrap().with_ymd_and_hms(2024, 1, 15, 0, 30, 0).unwrap();
    let p = RegexDateParser::new(cfg);
    assert!(p.filter_date(Some(utc(2024, 1, 14, 23, 0, 0))).is_some());
    assert!(p.filter_date(Some(utc(2024, 1, 14, 23, 31, 0))).is_none());
}
