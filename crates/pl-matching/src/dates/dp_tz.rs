// upstream: dateparser 1.4.3 timezone_parser.py (pop_tz_offset_from_string, _tz_offsets),
//           utils/__init__.py (get_timezone_from_tz_string, localize_timezone,
//           apply_timezone, apply_dateparser_timezone); pytz `localize(is_dst=False)`.

use std::str::FromStr;
use std::sync::{LazyLock, OnceLock};

use chrono::{DateTime, FixedOffset, NaiveDateTime, Offset, TimeZone};
use chrono_tz::{OffsetComponents, Tz};
use regex::{Regex, RegexSet};

use super::dp_locale::DATA;

fn py_tz_pattern(p: &str) -> String {
    // `$` is only used as end anchor in the tz table; inputs contain no newlines
    p.replace('$', r"\z")
}

/// Prefilter + full patterns of the time zone table.
///
/// Memory (measured 2026-11-18): a `RegexSet` of all 773 full patterns
/// (`(\W|\d|_)NAME($|\W)` or `(.)NAME$`, Unicode `\W`/`\d` twice per pattern) took
/// ~47 MiB plus ~7 MiB search buffer per concurrently parsing thread. The prefilter therefore contains
/// only the names (`NAME`, 1.3 MiB); every hit is a superset of the full hits (a
/// full pattern only matches where its name matches). The candidates are checked in table order
/// with the full pattern (translated individually and only when needed, ~30 KiB each) – same
/// result as before: the first pattern of the table that matches.
struct TzTable {
    set: RegexSet,
    regexes: Vec<OnceLock<Option<Regex>>>,
}

/// Name from a full table pattern (both forms of the dateparser table); otherwise the whole
/// pattern (then the prefilter is exact for this entry).
fn tz_name_pattern(p: &str) -> &str {
    p.strip_prefix(r"(\W|\d|_)")
        .and_then(|b| b.strip_suffix(r"($|\W)"))
        .or_else(|| p.strip_prefix("(.)").and_then(|b| b.strip_suffix('$')))
        .filter(|b| !b.is_empty() && !b.ends_with('\\'))
        .unwrap_or(p)
}

static TZ: LazyLock<Option<TzTable>> = LazyLock::new(|| {
    let pats: Vec<String> = DATA.tz.iter().map(|(_, p, _)| format!("(?i){}", py_tz_pattern(tz_name_pattern(p)))).collect();
    let set = regex::RegexSetBuilder::new(&pats).size_limit(256 * 1024 * 1024).build().ok()?;
    Some(TzTable { set, regexes: (0..pats.len()).map(|_| OnceLock::new()).collect() })
});

fn tz_regex(i: usize) -> Option<&'static Regex> {
    let t = TZ.as_ref()?;
    t.regexes[i].get_or_init(|| regex::RegexBuilder::new(&py_tz_pattern(&DATA.tz[i].1)).case_insensitive(true).build().ok()).as_ref()
}

/// First tz table entry (in table order) whose regex matches `s`, with its span.
fn first_tz_match(s: &str) -> Option<(usize, usize, usize)> {
    let t = TZ.as_ref()?;
    t.set.matches(s).iter().find_map(|i| tz_regex(i)?.find(s).map(|m| (i, m.start(), m.end())))
}

/// `pop_tz_offset_from_string(date_string)` -> (string, Some(offset_seconds, name)).
pub(crate) fn pop_tz_offset(s: &str) -> (String, Option<(i64, String)>) {
    match first_tz_match(s) {
        Some((i, start, stop)) => {
            let first_len = s[start..].chars().next().map_or(0, char::len_utf8);
            let out = format!("{}{}", &s[..start + first_len], &s[stop..]);
            (out, Some((DATA.tz[i].2, DATA.tz[i].0.clone())))
        }
        None => (s.to_string(), None),
    }
}

/// pytz.timezone() accepts names case-insensitively.
fn tz_insensitive(name: &str) -> Option<Tz> {
    chrono_tz::TZ_VARIANTS.iter().copied().find(|t| t.name().eq_ignore_ascii_case(name))
}

/// A resolved `settings.TIMEZONE`.
#[derive(Debug, Clone)]
pub(crate) enum SettingsTz {
    Db(Tz),
    Static(i64),
}

/// `get_timezone_from_tz_string`: pytz name first, then dateparser abbreviations.
pub(crate) fn resolve_tz(name: &str) -> Option<SettingsTz> {
    if let Ok(tz) = Tz::from_str(name) {
        return Some(SettingsTz::Db(tz));
    }
    if let Some(tz) = tz_insensitive(name) {
        return Some(SettingsTz::Db(tz));
    }
    first_tz_match(&format!(" {name}")).map(|(i, _, _)| SettingsTz::Static(DATA.tz[i].2))
}

fn fixed(secs: i64) -> FixedOffset {
    FixedOffset::east_opt(secs as i32).unwrap_or_else(|| chrono::Utc.fix())
}

/// pytz `tz.localize(naive)` with is_dst=False.
pub(crate) fn localize(naive: NaiveDateTime, tz: &SettingsTz) -> DateTime<FixedOffset> {
    match tz {
        SettingsTz::Static(s) => fixed(*s).from_local_datetime(&naive).single().unwrap_or_else(|| fixed(0).from_utc_datetime(&naive)),
        SettingsTz::Db(tz) => {
            let off = match tz.offset_from_local_datetime(&naive) {
                chrono::LocalResult::Single(o) => o.fix(),
                chrono::LocalResult::Ambiguous(a, b) => {
                    // is_dst=False -> the standard-time candidate
                    if a.dst_offset().is_zero() { a.fix() } else { b.fix() }
                }
                chrono::LocalResult::None => {
                    // non-existent wall time: pytz is_dst=False uses the offset in
                    // effect before the transition
                    let before = naive - chrono::Duration::hours(3);
                    tz.offset_from_local_datetime(&before).earliest().map_or(fixed(0), |o| o.fix())
                }
            };
            off.from_local_datetime(&naive).single().unwrap_or_else(|| off.from_utc_datetime(&naive))
        }
    }
}

/// `apply_timezone(date_time, tz_string)` for an aware datetime.
pub(crate) fn apply_timezone(dt: DateTime<FixedOffset>, tz_string: &str) -> DateTime<FixedOffset> {
    if let Some((i, _, _)) = first_tz_match(&format!(" {tz_string}")) {
        return dt.with_timezone(&fixed(DATA.tz[i].2));
    }
    match Tz::from_str(tz_string).ok().or_else(|| tz_insensitive(tz_string)).ok_or(()) {
        Ok(tz) => {
            let t = dt.with_timezone(&tz);
            t.with_timezone(&t.offset().fix())
        }
        Err(_) => dt,
    }
}

pub(crate) fn fixed_offset(secs: i64) -> FixedOffset {
    fixed(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pop_offsets() {
        let (s, tz) = pop_tz_offset("10:00 EST");
        assert_eq!(s, "10:00 ");
        assert_eq!(tz.map(|t| t.0), Some(-5 * 3600));
        let (s, tz) = pop_tz_offset("15 january 2024");
        assert_eq!(s, "15 january 2024");
        assert!(tz.is_none());
        assert!(matches!(resolve_tz("UTC"), Some(SettingsTz::Db(_))));
        assert!(matches!(resolve_tz("Europe/Berlin"), Some(SettingsTz::Db(_))));
    }

    /// Prefilter with names only: same result as the first matching full pattern of the table.
    #[test]
    fn name_prefilter_equals_full_patterns() {
        assert_eq!(tz_name_pattern(r"(\W|\d|_)EST($|\W)"), "EST");
        assert_eq!(tz_name_pattern(r"(.)UTC\+10:30$"), r"UTC\+10:30");
        assert_eq!(tz_name_pattern(r"(.)X\$"), r"(.)X\$");
        let full: Vec<Regex> =
            DATA.tz.iter().map(|(_, p, _)| regex::RegexBuilder::new(&py_tz_pattern(p)).case_insensitive(true).build().unwrap()).collect();
        for s in [
            "10:00 EST",
            "12. märz 2024 10:00 mez",
            "2024-03-12t10:00:00z",
            "15 january 2024",
            "12.03.2024 utc+05:30",
            "march 3 2024 1:00 pm pst",
            "zest 2024 aest",
            "3 mai 2024 cest",
            "_EST1",
            "ESTEST",
        ] {
            let want = full.iter().enumerate().find_map(|(i, r)| r.find(s).map(|m| (i, m.start(), m.end())));
            assert_eq!(first_tz_match(s), want, "{s}");
        }
    }
}
