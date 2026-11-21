//! Parity against upstream Python (dateparser 1.4.3 / rapidfuzz / fnmatch).
//! Reference: tests/parity/pl-matching/dates_reference.json, produced by
//! tests/parity/pl-matching/dates.py with reference/venv. Skips if the file is missing.

use std::collections::HashSet;

use chrono::{DateTime, FixedOffset, Timelike};
use pl_matching::dates::{DateParserConfig, RegexDateParser, date_regex_matches};
use serde_json::Value;

fn reference() -> Option<Value> {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/parity/pl-matching/dates_reference.json");
    let s = std::fs::read_to_string(p).ok()?;
    serde_json::from_str(&s).ok()
}

fn iso(d: &DateTime<FixedOffset>) -> String {
    let base = d.format("%Y-%m-%dT%H:%M:%S").to_string();
    let us = d.nanosecond() / 1000;
    let frac = if us != 0 { format!(".{us:06}") } else { String::new() };
    format!("{base}{frac}{}", d.format("%:z"))
}

fn config(cfg: &Value, now: DateTime<FixedOffset>, ignore: bool) -> DateParserConfig {
    let mut ignore_dates = HashSet::new();
    if ignore {
        ignore_dates.insert(chrono::NaiveDate::from_ymd_opt(2023, 12, 1).unwrap());
    }
    DateParserConfig {
        languages: cfg["languages"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect(),
        timezone_str: cfg["tz"].as_str().unwrap().into(),
        ignore_dates,
        reference_time: now,
        filename_date_order: cfg["fname_order"].as_str().map(str::to_string),
        content_date_order: cfg["order"].as_str().unwrap().into(),
    }
}

#[test]
fn dates_parity() {
    let Some(r) = reference() else {
        eprintln!("dates_reference.json missing - skipped");
        return;
    };
    let now = DateTime::parse_from_rfc3339(r["reference_now"].as_str().unwrap()).unwrap();
    let configs = r["configs"].as_array().unwrap();
    let parsers_ign: Vec<RegexDateParser> = configs.iter().map(|c| RegexDateParser::new(config(c, now, true))).collect();
    let parsers: Vec<RegexDateParser> = configs.iter().map(|c| RegexDateParser::new(config(c, now, false))).collect();

    let mut failures = Vec::new();
    let mut total = 0;
    for case in r["parse"].as_array().unwrap() {
        total += 1;
        let ci = case["config"].as_u64().unwrap() as usize;
        let fname = case["filename"].as_str().unwrap();
        let content = case["content"].as_str().unwrap();
        let expected_matches: Vec<&str> = case["regex_matches"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        let got_matches = date_regex_matches(content);
        if got_matches != expected_matches {
            failures.push(format!("REGEX {content:?}: got {got_matches:?} want {expected_matches:?}"));
        }
        let expected: Vec<&str> = case["dates_full"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        let got: Vec<String> = parsers_ign[ci].parse(fname, content).map(|d| iso(&d)).collect();
        if got != expected {
            failures.push(format!("PARSE cfg{ci} {fname:?} {content:?}: got {got:?} want {expected:?}"));
        }
    }
    for case in r["parse_string"].as_array().unwrap() {
        total += 1;
        let ci = case["config"].as_u64().unwrap() as usize;
        let s = case["string"].as_str().unwrap();
        let order = case["order"].as_str().unwrap();
        let expected = case["result"].as_str();
        let got = parsers[ci].parse_string(s, order).map(|d| iso(&d));
        if got.as_deref() != expected {
            failures.push(format!("STRING cfg{ci} {s:?}: got {got:?} want {expected:?}"));
        }
    }
    for case in r["fuzz"].as_array().unwrap() {
        total += 1;
        let (a, b) = (case["a"].as_str().unwrap(), case["b"].as_str().unwrap());
        let s = pl_matching::fuzz::partial_ratio(a, b, 0.0);
        let s90 = pl_matching::fuzz::partial_ratio(a, b, 90.0);
        if s != case["score"].as_f64().unwrap() || s90 != case["cutoff90"].as_f64().unwrap() {
            failures.push(format!("FUZZ {a:?} {b:?}: got {s}/{s90} want {}/{}", case["score"], case["cutoff90"]));
        }
    }
    for case in r["fnmatch"].as_array().unwrap() {
        total += 1;
        let (p, n) = (case["pattern"].as_str().unwrap(), case["name"].as_str().unwrap());
        let t = pl_matching::fnmatch::translate(p);
        let m = pl_matching::fnmatch::fnmatch(n, p);
        if t != case["translate"].as_str().unwrap() || m != case["match"].as_bool().unwrap() {
            failures.push(format!("FNMATCH {p:?} {n:?}: got {t} {m} want {} {}", case["translate"], case["match"]));
        }
    }
    assert!(failures.is_empty(), "{} of {total} cases differ:\n{}", failures.len(), failures.join("\n"));
}
