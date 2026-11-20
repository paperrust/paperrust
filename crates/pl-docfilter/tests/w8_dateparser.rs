//! W8: relative date words against dateparser 1.4.3 (version from upstream/uv.lock).
//!
//! `tests/data/w8_dateparser.tsv` was generated with dateparser itself: per row language, expression and
//! the result of `dateparser.parse(expr, locales=[lang, "en"], settings={"PARSERS":
//! ["relative-time"], "RELATIVE_BASE": 2024-08-22 23:30 UTC, …}).date()` (`None` = no match),
//! as `DateQueryField.to_internal_value` (upstream src/documents/filters.py) calls it.
//! Generated with `tools/gen_w8_corpus.py` (candidates from the dateparser language files
//! `date_translation_data/<lang>.py`: units, `ago`, `in`, number words, `relative-type`,
//! `relative-type-regex`, several word orders, edge cases; fixed selection of 160 per language).
//! With `--full` (16,429 rows) and with the earlier corpus from the old word lists (26,667 rows)
//! every row matched at creation time. Rows with `#skip` at the end would be skipped
//! (deliberate deviation, to be justified in the contract) – currently none.

use chrono::NaiveDate;
use pl_docfilter::dates::parse_relative_date;

#[test]
fn w8_word_lists_match_dateparser() {
    let data = include_str!("data/w8_dateparser.tsv");
    let now = NaiveDate::from_ymd_opt(2024, 8, 22).and_then(|d| d.and_hms_opt(23, 30, 0)).expect("valid base date");
    let mut bad = Vec::new();
    let mut n = 0;
    for line in data.lines() {
        if line.is_empty() || line.starts_with('#') || line.ends_with("#skip") {
            continue;
        }
        let mut it = line.split('\t');
        let (Some(lang), Some(expr), Some(want)) = (it.next(), it.next(), it.next()) else {
            panic!("bad corpus line {line:?}");
        };
        n += 1;
        let got = parse_relative_date(expr, now, Some(&[lang.to_string()]))
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "None".into());
        if got != want {
            bad.push(format!("{lang}\t{expr}\tdateparser={want}\tours={got}"));
        }
    }
    assert!(n > 0);
    assert!(bad.is_empty(), "{} of {n} differ:\n{}", bad.len(), bad.join("\n"));
}
