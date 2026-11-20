// whoosh_compat/parser/dateparse.py (whoosh-compat 0.3.0)
//
// The English date grammar (Sequence/Combo/Choice/Bag/Regex elements) and
// DateParserPlugin (do_date_phrases, do_unquoted_date_values, do_dates).
// Grammar regexes are matched anchored on the text slice; trailing
// look-aheads (`(?=(\W|$))`, `(?!:)`, ...) are folded into a consumed tail
// and the match end is taken before it (same accepted language and the
// same leftmost-first choice as Python's backtracking engine).
// dateutil's relativedelta is reimplemented for the (years, months, weeks,
// days, hours, minutes, seconds) form used here. Python exceptions:
// TimeError -> DErr::Time, ValueError/OverflowError -> DErr::Value.

use std::sync::OnceLock;

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Offset, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use regex::{Captures, Regex};

use super::plugins::py_space;
use super::syntax::{SNode, TextKind};
use super::times::{
    add_delta, disambiguated, fill_in, has_no_time_adt, is_ambiguous, is_void, py_check, relative_days, Adt, DErr,
    DResult, Span, SpanKind, DL, DV,
};
use crate::wc::errors::{Diagnostic, DiagnosticKind};
use crate::wc::fields::{FieldKind, FieldRef, FieldRegistry, FieldSpec};

// -- relativedelta ----------------------------------------------------------

#[derive(Default, Clone, Copy)]
struct RelDelta {
    years: i64,
    months: i64,
    weeks: i64,
    days: i64,
    hours: i64,
    minutes: i64,
    seconds: i64,
}

fn add_months_date(y: i64, m: i64, d: i64, months: i64) -> DResult<NaiveDate> {
    let total = y.checked_mul(12).and_then(|t| t.checked_add(m - 1)).and_then(|t| t.checked_add(months)).ok_or(DErr::Value)?;
    let ny = total.div_euclid(12);
    let nm = total.rem_euclid(12) + 1;
    if !(1..=9999).contains(&ny) {
        return Err(DErr::Value);
    }
    let dim = super::times::days_in_month(ny, nm)?;
    NaiveDate::from_ymd_opt(ny as i32, nm as u32, d.min(dim) as u32).ok_or(DErr::Value)
}

/// `dt + relativedelta(...)`
fn rd_add(dt: NaiveDateTime, rd: RelDelta) -> DResult<NaiveDateTime> {
    let months = rd.years.checked_mul(12).and_then(|y| y.checked_add(rd.months)).ok_or(DErr::Value)?;
    let date = add_months_date(i64::from(dt.year()), i64::from(dt.month()), i64::from(dt.day()), months)?;
    let base = NaiveDateTime::new(date, dt.time());
    let days = rd.weeks.checked_mul(7).and_then(|w| w.checked_add(rd.days)).ok_or(DErr::Value)?;
    let secs = rd
        .hours
        .checked_mul(3600)
        .and_then(|h| rd.minutes.checked_mul(60).and_then(|m| h.checked_add(m)))
        .and_then(|s| s.checked_add(rd.seconds))
        .ok_or(DErr::Value)?;
    if days.abs() > 999_999_999 {
        return Err(DErr::Value);
    }
    let delta = Duration::try_days(days)
        .and_then(|d| Duration::try_seconds(secs).and_then(|s| d.checked_add(&s)))
        .ok_or(DErr::Value)?;
    add_delta(base, delta)
}

fn date_add_days(d: NaiveDate, days: i64) -> DResult<NaiveDate> {
    let r = d.checked_add_signed(Duration::try_days(days).ok_or(DErr::Value)?).ok_or(DErr::Value)?;
    if (1..=9999).contains(&r.year()) {
        Ok(r)
    } else {
        Err(DErr::Value)
    }
}

// -- grammar elements -------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum RxKind {
    /// default props_to_date: adatetime from the named unit groups
    Units,
    Day,
    Year,
    Time24,
    Time12,
    Plus,
    NowCompact,
    Dayname,
    Midnight,
    Noon,
    Now,
    Tomorrow,
    Yesterday,
    ThisYear,
    ThisMonth,
    Today,
    PrevYear,
    PrevMonth,
    PrevWeek,
    PrevQuarter,
    YearMonth,
    Month,
}

struct Rx {
    re: Option<Regex>,
    kind: RxKind,
}

enum El {
    Rx(Rx),
    /// `Regex(r"(?=(\s|$))")`
    LookWsEnd,
    Seq { els: Vec<El>, sep: Option<Regex>, progressive: bool, simple_sep_ok: bool },
    Combo { els: Vec<El>, sep: Option<Regex> },
    Choice(Vec<El>),
    Bag { els: Vec<El>, sep: Option<Regex> },
}

const DEFAULT_SEP: &str = r"(\s+|\s*,\s*)";
const W_TAIL: &str = r"(?:\W|\z)";

fn rx(pat: &str, tail: &str, ci: bool, kind: RxKind) -> El {
    let flags = if ci { "(?i)" } else { "" };
    let re = Regex::new(&format!(r"{flags}\A(?P<zzm>{pat}){tail}")).ok();
    El::Rx(Rx { re, kind })
}

fn sep_re(pat: &str) -> Option<Regex> {
    Regex::new(&format!(r"(?i)\A(?:{pat})")).ok()
}

fn seq(els: Vec<El>) -> El {
    El::Seq { els, sep: sep_re(DEFAULT_SEP), progressive: false, simple_sep_ok: false }
}

const MONTHS: [&str; 12] = [
    "january|jan",
    "february|febuary|feb",
    "march|mar",
    "april|apr",
    "may",
    "june|jun",
    "july|jul",
    "august|aug",
    "september|sept|sep",
    "october|oct",
    "november|nov",
    "december|dec",
];

const DAYNAMES: [&str; 7] = [
    "monday|mon|mo",
    "tuesday|tues|tue|tu",
    "wednesday|wed|we",
    "thursday|thur|thu|th",
    "friday|fri|fr",
    "saturday|sat|sa",
    "sunday|sun|su",
];

/// First pattern index whose alternatives match the start of `text`
/// case-insensitively (`re.match(pat, text, re.IGNORECASE)`).
fn first_prefix_index(pats: &[&str], text: &str) -> Option<usize> {
    let lower = text.to_lowercase();
    pats.iter().position(|p| p.split('|').any(|alt| lower.starts_with(alt)))
}

fn day_el() -> El {
    // English.day: (?P<day>([123][0-9])|[1-9])(st|nd|rd|th)?(?=(\W|$))(?!:)
    rx(r"(?P<day>([123][0-9])|[1-9])(st|nd|rd|th)?", r"(?:[^\w:]|\z)", true, RxKind::Day)
}
fn year_el() -> El {
    rx(r"(?P<year>[0-9]{4})", W_TAIL, true, RxKind::Year)
}
fn month_el() -> El {
    let pat = format!("(?P<month>{})", MONTHS.iter().map(|p| format!("({p})")).collect::<Vec<_>>().join("|"));
    rx(&pat, "", true, RxKind::Month)
}

/// English grammar (`English.setup`): returns `self.all` and `self.time`.
struct Grammar {
    all: El,
    time: El,
}

fn time_el() -> El {
    El::Choice(vec![
        rx(
            r"(?P<hour>[1-9]|10|11|12)(:(?P<mins>[0-5][0-9])(:(?P<secs>[0-5][0-9])(\.(?P<usecs>[0-9]{1,5}))?)?)?\s*(?P<ampm>am|pm)",
            W_TAIL,
            true,
            RxKind::Time12,
        ),
        rx(
            r"(?P<hour>([0-1][0-9])|(2[0-3])):(?P<mins>[0-5][0-9])(:(?P<secs>[0-5][0-9])(\.(?P<usecs>[0-9]{1,5}))?)?",
            W_TAIL,
            true,
            RxKind::Time24,
        ),
        rx("midnight", "", true, RxKind::Midnight),
        rx("noon", "", true, RxKind::Noon),
        rx("now", "", true, RxKind::Now),
    ])
}

fn build_grammar() -> Grammar {
    let rel = |name: &str, units: &str| format!("((?P<{name}>[0-9]+) *({units}))?");
    let plus_pat = format!(
        "(?P<dir>[+-]) *{} *{} *{} *{} *{} *{} *{}",
        rel("years", "years|year|yrs|yr|ys|y"),
        rel("months", "months|month|mons|mon|mos|mo"),
        rel("weeks", "weeks|week|wks|wk|ws|w"),
        rel("days", "days|day|dys|dy|ds|d"),
        rel("hours", "hours|hour|hrs|hr|hs|h"),
        rel("mins", "minutes|minute|mins|min|ms|m"),
        rel("secs", "seconds|second|secs|sec|s"),
    );
    let plusdate = rx(&plus_pat, W_TAIL, true, RxKind::Plus);
    let nowcompact = rx(r"now(?P<sign>[+-])(?P<n>[0-9]+)(?P<unit>[yMwdhms])", W_TAIL, false, RxKind::NowCompact);
    let dayname = rx(
        &format!("(?P<dir>next|last) +(?P<day>{})", DAYNAMES.join("|")),
        W_TAIL,
        true,
        RxKind::Dayname,
    );

    let dmy = El::Choice(vec![
        seq(vec![day_el(), month_el(), year_el()]),
        seq(vec![month_el(), day_el(), year_el()]),
        seq(vec![year_el(), month_el(), day_el()]),
        seq(vec![year_el(), day_el(), month_el()]),
        seq(vec![day_el(), month_el()]),
        seq(vec![month_el(), day_el()]),
        seq(vec![month_el(), year_el()]),
        month_el(),
        rx(r"(?P<year>[0-9]{4})[-/. ](?P<month>[0-1][0-9])", r"(?:\s|\z)", true, RxKind::YearMonth),
        year_el(),
        dayname,
        rx("tomorrow", "", true, RxKind::Tomorrow),
        rx("yesterday", "", true, RxKind::Yesterday),
        rx("previous week", "", true, RxKind::PrevWeek),
        rx("previous quarter", "", true, RxKind::PrevQuarter),
        rx("previous month", "", true, RxKind::PrevMonth),
        rx("previous year", "", true, RxKind::PrevYear),
        rx("this year", "", true, RxKind::ThisYear),
        rx("this month", "", true, RxKind::ThisMonth),
        rx("today", "", true, RxKind::Today),
        rx("now", "", true, RxKind::Now),
    ]);
    let datetime = El::Bag { els: vec![time_el(), dmy], sep: sep_re(DEFAULT_SEP) };

    // DateParser.__init__: simple
    let units = [
        "(?P<year>[0-9]{4})",
        "(?P<month>[0-1][0-9])",
        "(?P<day>[0-3][0-9])",
        "(?P<hour>([0-1][0-9])|(2[0-3]))",
        "(?P<minute>[0-5][0-9])",
        "(?P<second>[0-5][0-9])",
        "(?P<microsecond>[0-9]{6})",
    ];
    let simple_seq = El::Seq {
        els: units.iter().map(|u| rx(u, "", true, RxKind::Units)).collect(),
        sep: sep_re("[- .:/T]*"),
        progressive: true,
        simple_sep_ok: true,
    };
    let simple = El::Seq { els: vec![simple_seq, El::LookWsEnd], sep: None, progressive: false, simple_sep_ok: false };

    let bundle = || El::Choice(vec![
        rx(r"now(?P<sign>[+-])(?P<n>[0-9]+)(?P<unit>[yMwdhms])", W_TAIL, false, RxKind::NowCompact),
        rx(&plus_pat, W_TAIL, true, RxKind::Plus),
        El::Seq {
            els: vec![
                El::Seq {
                    els: units.iter().map(|u| rx(u, "", true, RxKind::Units)).collect(),
                    sep: sep_re("[- .:/T]*"),
                    progressive: true,
                    simple_sep_ok: true,
                },
                El::LookWsEnd,
            ],
            sep: None,
            progressive: false,
            simple_sep_ok: false,
        },
        build_datetime(),
    ]);
    let first_bundle = El::Choice(vec![nowcompact, plusdate, simple, datetime]);
    let torange = El::Combo { els: vec![bundle(), rx("to", "", true, RxKind::Units), bundle()], sep: sep_re(DEFAULT_SEP) };
    Grammar { all: El::Choice(vec![torange, first_bundle]), time: time_el() }
}

/// A fresh `self.datetime` Bag (elements are not shareable in this tree).
fn build_datetime() -> El {
    let dayname = rx(
        &format!("(?P<dir>next|last) +(?P<day>{})", DAYNAMES.join("|")),
        W_TAIL,
        true,
        RxKind::Dayname,
    );
    let dmy = El::Choice(vec![
        seq(vec![day_el(), month_el(), year_el()]),
        seq(vec![month_el(), day_el(), year_el()]),
        seq(vec![year_el(), month_el(), day_el()]),
        seq(vec![year_el(), day_el(), month_el()]),
        seq(vec![day_el(), month_el()]),
        seq(vec![month_el(), day_el()]),
        seq(vec![month_el(), year_el()]),
        month_el(),
        rx(r"(?P<year>[0-9]{4})[-/. ](?P<month>[0-1][0-9])", r"(?:\s|\z)", true, RxKind::YearMonth),
        year_el(),
        dayname,
        rx("tomorrow", "", true, RxKind::Tomorrow),
        rx("yesterday", "", true, RxKind::Yesterday),
        rx("previous week", "", true, RxKind::PrevWeek),
        rx("previous quarter", "", true, RxKind::PrevQuarter),
        rx("previous month", "", true, RxKind::PrevMonth),
        rx("previous year", "", true, RxKind::PrevYear),
        rx("this year", "", true, RxKind::ThisYear),
        rx("this month", "", true, RxKind::ThisMonth),
        rx("today", "", true, RxKind::Today),
        rx("now", "", true, RxKind::Now),
    ]);
    El::Bag { els: vec![time_el(), dmy], sep: sep_re(DEFAULT_SEP) }
}

fn grammar() -> &'static Grammar {
    static G: OnceLock<Grammar> = OnceLock::new();
    G.get_or_init(build_grammar)
}

/// dateparse.py::_simple_sep_ok
fn simple_sep_ok(index: usize, sep: &str, earlier: &[String]) -> bool {
    const DAY: usize = 2;
    const HOUR: usize = 3;
    const MINUTE: usize = 4;
    const SECOND: usize = 5;
    if sep.contains(':') {
        return index == MINUTE || index == SECOND;
    }
    if sep.is_empty() && index <= HOUR {
        return !earlier.iter().any(|s| !s.is_empty());
    }
    if earlier.iter().any(|s| s.contains(' ')) {
        if index == HOUR {
            return sep.contains(' ') || sep == "T" || sep == "t";
        }
        if index == DAY {
            return !sep.contains('.');
        }
    }
    true
}

type PR = DResult<Option<(DV, usize)>>;

fn sep_match(sep: &Regex, text: &str, pos: usize) -> Option<usize> {
    sep.find(&text[pos..]).map(|m| pos + m.end())
}

fn int_group(c: &Captures<'_>, name: &str) -> DResult<Option<i64>> {
    match c.name(name) {
        None => Ok(None),
        Some(m) => m.as_str().parse::<i64>().map(Some).map_err(|_| DErr::Value),
    }
}

fn is_py_ws_only(s: &str) -> bool {
    s.chars().all(py_space)
}

impl El {
    fn parse(&self, text: &str, dt: NaiveDateTime, pos: usize) -> PR {
        match self {
            El::Rx(r) => r.parse(text, dt, pos),
            El::LookWsEnd => {
                let ok = text[pos..].chars().next().is_none_or(py_space);
                Ok(if ok { Some((DV::A(Adt::default()), pos)) } else { None })
            }
            El::Seq { els, sep, progressive, simple_sep_ok: sso } => {
                let mut pos = pos;
                let mut d = DV::A(Adt::default());
                let mut first = true;
                let mut foundall = true;
                let mut failed = false;
                let mut seps: Vec<String> = Vec::new();
                for (i, e) in els.iter().enumerate() {
                    let mut elempos = pos;
                    if let (Some(sep), false) = (sep, first) {
                        match sep.find(&text[pos..]) {
                            Some(m) => {
                                let s = m.as_str().to_string();
                                if *sso && !simple_sep_ok(i, &s, &seps) {
                                    foundall = false;
                                    break;
                                }
                                elempos = pos + m.end();
                                seps.push(s);
                                if is_py_ws_only(&text[pos..elempos]) {
                                    pos = elempos;
                                }
                            }
                            None => {
                                foundall = false;
                                break;
                            }
                        }
                    }
                    match e.parse(text, dt, elempos) {
                        Err(DErr::Time) => {
                            failed = true;
                            foundall = false;
                            break;
                        }
                        Err(DErr::Value) => return Err(DErr::Value),
                        Ok(None) => {
                            foundall = false;
                            break;
                        }
                        Ok(Some((at, np))) => {
                            pos = np;
                            match fill_in(&d, &at) {
                                Err(DErr::Time) => {
                                    failed = true;
                                    foundall = false;
                                    break;
                                }
                                Err(DErr::Value) => return Err(DErr::Value),
                                Ok(v) => d = v,
                            }
                        }
                    }
                    first = false;
                }
                if !failed && (foundall || (!first && *progressive)) {
                    Ok(Some((d, pos)))
                } else {
                    Ok(None)
                }
            }
            El::Combo { els, sep } => {
                let mut pos = pos;
                let mut dates: Vec<DV> = Vec::new();
                let mut first = true;
                for e in els {
                    if !first {
                        match sep.as_ref().and_then(|s| sep_match(s, text, pos)) {
                            Some(p) => pos = p,
                            None => return Ok(None),
                        }
                    }
                    let r = match e.parse(text, dt, pos) {
                        Err(DErr::Time) => None,
                        Err(DErr::Value) => return Err(DErr::Value),
                        Ok(r) => r,
                    };
                    let Some((at, np)) = r else { return Ok(None) };
                    pos = np;
                    first = false;
                    if is_void(&at) {
                        continue;
                    }
                    if dates.len() == 2 {
                        return Ok(None);
                    }
                    dates.push(at);
                }
                if dates.len() < 2 {
                    return Ok(None);
                }
                // Combo.dates_to_timespan -> timespan(a, b): TimeError unless
                // both are datetime/adatetime.
                let to_dl = |v: &DV| match v {
                    DV::Dt(x) => Some(DL::Dt(*x)),
                    DV::A(a) => Some(DL::A(*a)),
                    DV::S(_) => None,
                };
                match (to_dl(&dates[0]), to_dl(&dates[1])) {
                    (Some(a), Some(b)) => Ok(Some((DV::S(Span::new(a, b)), pos))),
                    _ => Err(DErr::Time),
                }
            }
            El::Choice(els) => {
                for e in els {
                    match e.parse(text, dt, pos) {
                        Err(DErr::Time) | Ok(None) => {}
                        Err(DErr::Value) => return Err(DErr::Value),
                        Ok(Some(r)) => return Ok(Some(r)),
                    }
                }
                Ok(None)
            }
            El::Bag { els, sep } => {
                let mut pos = pos;
                let mut first = true;
                let mut d = DV::A(Adt::default());
                let mut seen = vec![false; els.len()];
                loop {
                    let mut newpos = pos;
                    if !first {
                        match sep.as_ref().and_then(|s| sep_match(s, text, pos)) {
                            Some(p) => newpos = p,
                            None => break,
                        }
                    }
                    let mut matched = false;
                    for (i, e) in els.iter().enumerate() {
                        let r = match e.parse(text, dt, newpos) {
                            Err(DErr::Time) => None,
                            Err(DErr::Value) => return Err(DErr::Value),
                            Ok(r) => r,
                        };
                        if let Some((at, xpos)) = r {
                            if seen[i] {
                                return Ok(None);
                            }
                            d = fill_in(&d, &at)?;
                            newpos = xpos;
                            seen[i] = true;
                            matched = true;
                            break;
                        }
                    }
                    if !matched {
                        break;
                    }
                    pos = newpos;
                    if seen.iter().all(|s| *s) {
                        break;
                    }
                    first = false;
                }
                if !seen.iter().any(|s| *s) {
                    return Ok(None);
                }
                Ok(Some((d, pos)))
            }
        }
    }
}

impl Rx {
    fn parse(&self, text: &str, dt: NaiveDateTime, pos: usize) -> PR {
        let Some(re) = &self.re else { return Ok(None) };
        let Some(c) = re.captures(&text[pos..]) else { return Ok(None) };
        let Some(m) = c.name("zzm") else { return Ok(None) };
        let end = pos + m.end();
        match self.props_to_date(&c, dt) {
            Err(DErr::Time) => Ok(None),
            Err(DErr::Value) => Err(DErr::Value),
            Ok(None) => Ok(None),
            Ok(Some(d)) => Ok(Some((d, end))),
        }
    }

    fn props_to_date(&self, c: &Captures<'_>, dt: NaiveDateTime) -> DResult<Option<DV>> {
        let today = || dt.date();
        let ymd = |d: NaiveDate| -> DResult<DV> {
            Ok(DV::A(Adt::ymd(i64::from(d.year()), i64::from(d.month()), i64::from(d.day()))?))
        };
        let g = |n: &str| int_group(c, n);
        let v = match self.kind {
            RxKind::Units => DV::A(Adt::new(
                g("year")?,
                g("month")?,
                g("day")?,
                g("hour")?,
                g("minute")?,
                g("second")?,
                g("microsecond")?,
            )?),
            RxKind::Day => DV::A(Adt::new(None, None, g("day")?, None, None, None, None)?),
            RxKind::Year => DV::A(Adt::new(g("year")?, None, None, None, None, None, None)?),
            RxKind::YearMonth => DV::A(Adt::new(g("year")?, g("month")?, None, None, None, None, None)?),
            RxKind::Month => {
                let text = c.name("month").map_or("", |m| m.as_str());
                let Some(i) = first_prefix_index(&MONTHS, text) else { return Ok(None) };
                DV::A(Adt::new(None, Some(i as i64 + 1), None, None, None, None, None)?)
            }
            RxKind::Time24 => {
                DV::A(Adt::new(None, None, None, g("hour")?, g("mins")?, g("secs")?, g("usecs")?)?)
            }
            RxKind::Time12 => {
                let isam = c.name("ampm").is_some_and(|m| m.as_str().to_lowercase().starts_with('a'));
                let hour = g("hour")?.unwrap_or(0);
                let hr = if hour == 12 {
                    if isam {
                        0
                    } else {
                        12
                    }
                } else if isam {
                    hour
                } else {
                    hour + 12
                };
                DV::A(Adt::new(None, None, None, Some(hr), g("mins")?, g("secs")?, g("usecs")?)?)
            }
            RxKind::Plus => {
                let dir = if c.name("dir").is_some_and(|m| m.as_str() == "-") { -1 } else { 1 };
                let f = |n: &str| -> DResult<i64> { g(n)?.unwrap_or(0).checked_mul(dir).ok_or(DErr::Value) };
                let rd = RelDelta {
                    years: f("years")?,
                    months: f("months")?,
                    weeks: f("weeks")?,
                    days: f("days")?,
                    hours: f("hours")?,
                    minutes: f("mins")?,
                    seconds: f("secs")?,
                };
                DV::Dt(rd_add(dt, rd)?)
            }
            RxKind::NowCompact => {
                let dir = if c.name("sign").is_some_and(|m| m.as_str() == "-") { -1 } else { 1 };
                let n = g("n")?.unwrap_or(0).checked_mul(dir).ok_or(DErr::Value)?;
                let mut rd = RelDelta::default();
                match c.name("unit").map_or("", |m| m.as_str()) {
                    "y" => rd.years = n,
                    "M" => rd.months = n,
                    "w" => rd.weeks = n,
                    "d" => rd.days = n,
                    "h" => rd.hours = n,
                    "m" => rd.minutes = n,
                    _ => rd.seconds = n,
                }
                DV::Dt(rd_add(dt, rd)?)
            }
            RxKind::Dayname => {
                // re.match(p.dir, "last") is case-sensitive.
                let dir = if c.name("dir").is_some_and(|m| m.as_str() == "last") { -1 } else { 1 };
                let day = c.name("day").map_or("", |m| m.as_str());
                let daynum = first_prefix_index(&DAYNAMES, day).unwrap_or(DAYNAMES.len() - 1) as i64;
                let cur = i64::from(dt.weekday().num_days_from_monday());
                let d = date_add_days(today(), relative_days(cur, daynum, dir))?;
                ymd(d)?
            }
            RxKind::Midnight => DV::A(Adt::new(None, None, None, Some(0), Some(0), Some(0), Some(0))?),
            RxKind::Noon => DV::A(Adt::new(None, None, None, Some(12), Some(0), Some(0), Some(0))?),
            RxKind::Now => DV::Dt(dt),
            RxKind::Tomorrow => ymd(date_add_days(today(), 1)?)?,
            RxKind::Yesterday => ymd(date_add_days(today(), -1)?)?,
            RxKind::ThisYear => DV::A(Adt::new(Some(i64::from(dt.year())), None, None, None, None, None, None)?),
            RxKind::ThisMonth => DV::A(Adt::new(
                Some(i64::from(dt.year())),
                Some(i64::from(dt.month())),
                None,
                None,
                None,
                None,
                None,
            )?),
            RxKind::Today => ymd(today())?,
            RxKind::PrevYear => DV::A(Adt::new(Some(i64::from(dt.year()) - 1), None, None, None, None, None, None)?),
            RxKind::PrevMonth => {
                let p = add_months_date(i64::from(dt.year()), i64::from(dt.month()), 1, -1)?;
                DV::A(Adt::new(Some(i64::from(p.year())), Some(i64::from(p.month())), None, None, None, None, None)?)
            }
            RxKind::PrevWeek => {
                let this_monday = date_add_days(today(), -i64::from(dt.weekday().num_days_from_monday()))?;
                let start = NaiveDateTime::new(date_add_days(this_monday, -7)?, NaiveTime::MIN);
                let end_excl = NaiveDateTime::new(this_monday, NaiveTime::MIN);
                let end = add_delta(end_excl, Duration::microseconds(-1))?;
                DV::S(Span { start: DL::Dt(start), end: DL::Dt(end), bounds_swapped: false, kind: SpanKind::CalendarPeriod("week") })
            }
            RxKind::PrevQuarter => {
                let qm = ((i64::from(dt.month()) - 1) / 3) * 3 + 1;
                let qs = NaiveDate::from_ymd_opt(dt.year(), qm as u32, 1).ok_or(DErr::Value)?;
                let pqs = add_months_date(i64::from(qs.year()), i64::from(qs.month()), 1, -3)?;
                let start = NaiveDateTime::new(pqs, NaiveTime::MIN);
                let end = add_delta(NaiveDateTime::new(qs, NaiveTime::MIN), Duration::microseconds(-1))?;
                DV::S(Span { start: DL::Dt(start), end: DL::Dt(end), bounds_swapped: false, kind: SpanKind::CalendarPeriod("quarter") })
            }
        };
        Ok(Some(v))
    }
}

/// `ToEnd(element).parse(text, dt)` -> date
fn to_end(el: &El, text: &str, dt: NaiveDateTime) -> DResult<Option<DV>> {
    match el.parse(text, dt, 0) {
        Err(DErr::Time) => Ok(None),
        Err(DErr::Value) => Err(DErr::Value),
        Ok(Some((d, np))) if np == text.len() => Ok(Some(d)),
        Ok(_) => Ok(None),
    }
}

/// dateparse.py::_time_on_period
fn time_on_period(raw: &DV) -> Option<&'static str> {
    fn adt_gap(a: &Adt) -> Option<&'static str> {
        if a.day.is_none() && !has_no_time_adt(a) {
            if a.month.is_some() {
                return Some("month");
            }
            if a.year.is_some() {
                return Some("year");
            }
        }
        None
    }
    let dl = |d: &DL| match d {
        DL::A(a) => adt_gap(a),
        DL::Dt(_) => None,
    };
    match raw {
        DV::S(s) => {
            if let SpanKind::TimeOnPeriod(u) = s.kind {
                return Some(u);
            }
            dl(&s.start).or_else(|| dl(&s.end))
        }
        DV::A(a) => adt_gap(a),
        DV::Dt(_) => None,
    }
}

// -- plugin -------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum TzSel {
    Local,
    Utc,
}

/// dateparse.py::DateParserPlugin
pub struct DateParserPlugin {
    basedate: DateTime<Utc>,
    tz: Tz,
}

const KEYWORD_PHRASES: [(&str, &[&str]); 2] =
    [("previous", &["week", "month", "quarter", "year"]), ("this", &["month", "year"])];
const UNQUOTED_LOOKAHEAD: usize = 15;
const TO_SEP_CHARS: &str = " \t\n\r\u{c}\u{b},";

fn rfc3339_utc_re() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    // Python `$` also matches before a final "\n", which the body excludes.
    RE.get_or_init(|| Regex::new(r"(?i)\A(?P<body>[^\nT]*T[^\n]*)Z\n?\z").ok()).as_ref()
}

fn is_exact_text(v: &DL) -> bool {
    match v {
        DL::Dt(d) => super::times::micro(d) != 999_999,
        DL::A(a) => !is_ambiguous(&DL::A(*a)),
    }
}

fn dv_start(v: &DV) -> DL {
    match v {
        DV::S(s) => s.start,
        DV::Dt(d) => DL::Dt(*d),
        DV::A(a) => DL::A(*a),
    }
}
fn dv_end(v: &DV) -> DL {
    match v {
        DV::S(s) => s.end,
        DV::Dt(d) => DL::Dt(*d),
        DV::A(a) => DL::A(*a),
    }
}

fn dl_dt(v: &DL) -> DResult<NaiveDateTime> {
    match v {
        DL::Dt(d) => Ok(*d),
        DL::A(_) => Err(DErr::Value),
    }
}

impl DateParserPlugin {
    pub fn new(basedate: DateTime<Utc>, tz: Tz) -> Self {
        DateParserPlugin { basedate, tz }
    }

    /// `_local_now`: naive, in self.tz, microsecond precision.
    fn local_now(&self) -> NaiveDateTime {
        let n = self.basedate.with_timezone(&self.tz).naive_local();
        n.with_nanosecond(n.nanosecond() / 1000 * 1000).unwrap_or(n)
    }

    /// `dt.replace(tzinfo=zoneinfo).astimezone(UTC)` with fold=0.
    fn local_to_utc(&self, n: NaiveDateTime) -> DResult<DateTime<Utc>> {
        use chrono::LocalResult;
        let utc = match self.tz.from_local_datetime(&n) {
            LocalResult::Single(d) => d.with_timezone(&Utc),
            LocalResult::Ambiguous(a, b) => a.min(b).with_timezone(&Utc),
            LocalResult::None => {
                // Gap: fold=0 uses the offset in effect before the transition.
                let probe = n.checked_sub_signed(Duration::days(1)).ok_or(DErr::Value)?;
                let off = self.tz.offset_from_utc_datetime(&probe).fix();
                let u = n.checked_sub_signed(Duration::seconds(i64::from(off.local_minus_utc()))).ok_or(DErr::Value)?;
                Utc.from_utc_datetime(&u)
            }
        };
        py_check(utc.naive_utc())?;
        Ok(utc)
    }

    /// `_to_utc`
    fn to_utc(&self, dt: NaiveDateTime, date_only: bool, ceil: bool, tz: TzSel) -> DResult<DateTime<Utc>> {
        if date_only {
            let mut d = dt;
            if ceil && d.time() != NaiveTime::MIN {
                d = add_delta(d, Duration::days(1))?;
            }
            return Ok(Utc.from_utc_datetime(&NaiveDateTime::new(d.date(), NaiveTime::MIN)));
        }
        match tz {
            TzSel::Utc => Ok(Utc.from_utc_datetime(&dt)),
            TzSel::Local => self.local_to_utc(dt),
        }
    }

    /// `_split_rfc3339_utc`
    fn split_rfc3339_utc(text: &str) -> (String, bool) {
        if let Some(c) = rfc3339_utc_re().and_then(|re| re.captures(text)) {
            if let Some(b) = c.name("body") {
                return (b.as_str().to_string(), true);
            }
        }
        (text.to_string(), false)
    }

    /// `_find_to_split`
    fn find_to_split(text: &str) -> Option<(String, String)> {
        let lower = text.to_lowercase();
        // Non-ASCII: offsets of `lower` need not match `text` even with equal total length
        // (e.g. U+212A shrinks, U+0130 grows) → char-wise scan (byte slicing would panic).
        if !text.is_ascii() {
            let chars: Vec<char> = text.chars().collect();
            for i in 0..chars.len().saturating_sub(1) {
                let is_to = chars[i].to_lowercase().eq("t".chars()) && chars[i + 1].to_lowercase().eq("o".chars());
                if is_to && i > 0 && TO_SEP_CHARS.contains(chars[i - 1]) && chars.get(i + 2).is_some_and(|c| TO_SEP_CHARS.contains(*c)) {
                    let l: String = chars[..i].iter().collect();
                    let r: String = chars[i + 2..].iter().collect();
                    return Some((l.trim_end_matches(|c| TO_SEP_CHARS.contains(c)).to_string(), r.trim_start_matches(|c| TO_SEP_CHARS.contains(c)).to_string()));
                }
            }
            return None;
        }
        let mut start = 0;
        while let Some(off) = lower[start..].find("to") {
            let idx = start + off;
            let after = idx + 2;
            let before_ok = idx > 0 && text[..idx].chars().next_back().is_some_and(|c| TO_SEP_CHARS.contains(c));
            let after_ok = text[after..].chars().next().is_some_and(|c| TO_SEP_CHARS.contains(c));
            if before_ok && after_ok {
                return Some((
                    text[..idx].trim_end_matches(|c| TO_SEP_CHARS.contains(c)).to_string(),
                    text[after..].trim_start_matches(|c| TO_SEP_CHARS.contains(c)).to_string(),
                ));
            }
            start = idx + 1;
        }
        None
    }

    /// `_split_span_rfc3339_utc`
    fn split_span_rfc3339_utc(text: &str) -> (String, bool, bool) {
        match Self::find_to_split(text) {
            None => {
                let (b, u) = Self::split_rfc3339_utc(text);
                (b, u, u)
            }
            Some((l, r)) => {
                let (lb, lu) = Self::split_rfc3339_utc(&l);
                let (rb, ru) = Self::split_rfc3339_utc(&r);
                (format!("{lb} to {rb}"), lu, ru)
            }
        }
    }

    fn bad_date(text: &str, spec: &FieldSpec, message: String, sc: Option<usize>, ec: Option<usize>, suggestion: Option<String>) -> SNode {
        let mut d = Diagnostic::new(DiagnosticKind::BadDate, message);
        d.startchar = sc;
        d.endchar = ec;
        d.field = Some(FieldRef::plain(spec.name.clone()));
        d.field_kind = Some(spec.kind);
        d.raw_value = Some(text.to_string());
        d.suggestion = suggestion;
        SNode::DateError { diag: Box::new(d), sc, ec }
    }

    /// `_error` (span set by the caller)
    fn error(node: &SNode, text: &str, spec: &FieldSpec) -> SNode {
        let mut n = Self::bad_date(
            text,
            spec,
            format!("{} is not a recognizable date", crate::error::py_repr(text)),
            node.startchar(),
            node.endchar(),
            None,
        );
        n.set_range(None, None);
        n
    }

    fn time_on_period_error(text: &str, spec: &FieldSpec, unit: &str, sc: Option<usize>, ec: Option<usize>) -> SNode {
        let msg = format!(
            "{} pairs a time of day with a whole {unit}; name a day, or drop the time",
            crate::error::py_repr(text)
        );
        Self::bad_date(text, spec, msg, sc, ec, None)
    }

    fn unquoted_error(text: &str, spec: &FieldSpec, sc: Option<usize>, ec: Option<usize>) -> SNode {
        let quoted = format!("\"{text}\"");
        let msg = format!(
            "{} is a date value written without quotes; quote it as {}:{quoted}",
            crate::error::py_repr(text),
            spec.name
        );
        Self::bad_date(text, spec, msg, sc, ec, Some(quoted))
    }

    /// `_date_spec`
    fn date_spec<'r>(reg: &'r FieldRegistry, fname: Option<&str>) -> Option<&'r FieldSpec> {
        let r = reg.make_ref(fname?)?;
        let res = reg.resolve(&r)?;
        matches!(res.spec.kind, FieldKind::Date | FieldKind::Datetime).then_some(res.spec)
    }

    /// `_is_time_of_day`
    fn is_time_of_day(&self, text: &str) -> bool {
        matches!(to_end(&grammar().time, text, self.local_now()), Ok(Some(_)))
    }

    /// `_phrase_words`
    fn phrase_words(nodes: &[SNode], i: usize, limit: usize) -> Vec<usize> {
        let mut found = Vec::new();
        let mut j = i + 1;
        while found.len() < limit && j < nodes.len() {
            let n = &nodes[j];
            if n.is_ws() {
                j += 1;
                continue;
            }
            if !n.is_plain_word() || n.fieldname().is_some() {
                break;
            }
            found.push(j);
            j += 1;
        }
        found
    }

    /// `_phrase_run`
    fn phrase_run(&self, nodes: &[SNode], i: usize) -> Option<Vec<usize>> {
        let mut idxs = vec![i];
        idxs.extend(Self::phrase_words(nodes, i, 2));
        let words: Vec<String> = idxs.iter().map(|j| nodes[*j].text().unwrap_or_default().to_lowercase()).collect();
        if words.len() < 2 {
            return None;
        }
        let ok = KEYWORD_PHRASES.iter().any(|(h, s)| *h == words[0] && s.contains(&words[1].as_str()));
        if !ok {
            return None;
        }
        let mut end = 2;
        if words.len() > end && self.is_time_of_day(&words[end]) {
            end += 1;
        }
        idxs.truncate(end);
        Some(idxs)
    }

    /// `do_date_phrases`
    pub fn do_date_phrases(&self, group: &mut SNode, reg: &FieldRegistry) {
        let SNode::Group { nodes, .. } = group else { return };
        for n in nodes.iter_mut() {
            if n.is_group() {
                self.do_date_phrases(n, reg);
            }
        }
        let mut i = 0;
        while i < nodes.len() {
            let head = &nodes[i];
            if head.is_plain_word() && head.fieldname().is_some() && Self::date_spec(reg, head.fieldname()).is_some() {
                if let Some(idxs) = self.phrase_run(nodes, i) {
                    let last = idxs[idxs.len() - 1];
                    let joined_text = idxs.iter().map(|j| nodes[*j].text().unwrap_or_default()).collect::<Vec<_>>().join(" ");
                    let fname = head.fieldname().map(str::to_string);
                    let joined = SNode::Text {
                        kind: TextKind::Word,
                        text: joined_text,
                        fieldname: fname,
                        boost: 1.0,
                        sc: head.startchar(),
                        ec: nodes[last].endchar(),
                    };
                    nodes.splice(i..=last, [joined]);
                }
            }
            i += 1;
        }
    }

    /// `_raw_full_parse`
    fn raw_full_parse(&self, text: &str) -> Option<DV> {
        let now = self.local_now();
        let raw = to_end(&grammar().all, text, now).ok()??;
        if !matches!(raw, DV::Dt(_)) && disambiguated(&raw, now).is_err() {
            return None;
        }
        Some(raw)
    }

    /// `do_unquoted_date_values`
    pub fn do_unquoted_date_values(&self, group: &mut SNode, reg: &FieldRegistry) {
        let SNode::Group { nodes, .. } = group else { return };
        for n in nodes.iter_mut() {
            if n.is_group() {
                self.do_unquoted_date_values(n, reg);
            }
        }
        let mut i = 0;
        while i < nodes.len() {
            let head = &nodes[i];
            if !(head.is_plain_word() && head.fieldname().is_some()) {
                i += 1;
                continue;
            }
            let Some(spec) = Self::date_spec(reg, head.fieldname()) else {
                i += 1;
                continue;
            };
            let mut idxs = vec![i];
            idxs.extend(Self::phrase_words(nodes, i, UNQUOTED_LOOKAHEAD));
            // _whitespace_separated
            for n in 1..idxs.len() {
                if nodes[idxs[n - 1]].endchar() == nodes[idxs[n]].startchar() {
                    idxs.truncate(n);
                    break;
                }
            }
            let words: Vec<String> = idxs.iter().map(|j| nodes[*j].text().unwrap_or_default().to_string()).collect();
            let mut k = words.len();
            while k > 1 {
                let joined = words[..k].join(" ");
                if let Some(raw) = self.raw_full_parse(&joined) {
                    let last = idxs[k - 1];
                    let sc = nodes[i].startchar();
                    let ec = nodes[last].endchar();
                    let err = match time_on_period(&raw) {
                        None => Self::unquoted_error(&joined, spec, sc, ec),
                        Some(unit) => Self::time_on_period_error(&joined, spec, unit, sc, ec),
                    };
                    nodes.splice(i..=last, [err]);
                    break;
                }
                k -= 1;
            }
            i += 1;
        }
    }

    /// `do_dates`
    pub fn do_dates(&self, group: &mut SNode, reg: &FieldRegistry) {
        let SNode::Group { nodes, .. } = group else { return };
        for i in 0..nodes.len() {
            if nodes[i].is_group() {
                self.do_dates(&mut nodes[i], reg);
                continue;
            }
            let fname = if nodes[i].has_fieldname() { nodes[i].fieldname() } else { None };
            let Some(spec) = Self::date_spec(reg, fname) else { continue };
            let node = &nodes[i];
            let mut new_node = if let SNode::Range { .. } = node {
                self.range_to_node(node, spec)
            } else if node.has_text() {
                self.text_to_node(node, spec)
            } else {
                continue;
            };
            if matches!(new_node, SNode::DateError { .. }) && node.is_plain_word() {
                Self::widen_bad_date_error(&mut new_node, nodes, i);
            } else {
                new_node.set_range(node.startchar(), node.endchar());
            }
            nodes[i] = new_node;
        }
    }

    /// `_leftover_fragment_text`
    fn leftover_fragment_text(sib: &SNode) -> Option<String> {
        if sib.is_plain_word() && sib.fieldname().is_none() {
            return sib.text().map(str::to_string);
        }
        if let SNode::Group { nodes, .. } = sib {
            let first = nodes.first()?;
            if first.is_plain_word()
                && first.fieldname().is_some()
                && nodes.iter().all(|c| {
                    c.is_plain_word() && c.text() == first.text() && c.startchar() == first.startchar() && c.endchar() == first.endchar()
                })
            {
                return first.text().map(str::to_string);
            }
        }
        None
    }

    /// `_widen_bad_date_error`
    fn widen_bad_date_error(err: &mut SNode, nodes: &[SNode], i: usize) {
        let node = &nodes[i];
        let mut end = node.endchar();
        let mut extra = String::new();
        let mut j = i + 1;
        while j < nodes.len() {
            let sib = &nodes[j];
            if sib.startchar() == end {
                if let Some(frag) = Self::leftover_fragment_text(sib) {
                    extra.push_str(&frag);
                    end = sib.endchar();
                    j += 1;
                    continue;
                }
            }
            break;
        }
        if let SNode::DateError { diag, sc, ec } = err {
            *sc = node.startchar();
            *ec = end;
            if !extra.is_empty() {
                let raw = format!("{}{extra}", diag.raw_value.clone().unwrap_or_default());
                diag.message = format!("{} is not a recognizable date", crate::error::py_repr(&raw));
                diag.endchar = end;
                diag.raw_value = Some(raw);
            }
        }
    }

    /// `text_to_node`
    fn text_to_node(&self, node: &SNode, spec: &FieldSpec) -> SNode {
        let text = node.text().unwrap_or_default();
        match self.text_to_node_inner(node, spec, text) {
            Ok(n) => n,
            Err(_) => Self::error(node, text, spec),
        }
    }

    fn text_to_node_inner(&self, node: &SNode, spec: &FieldSpec, text: &str) -> DResult<SNode> {
        let local_now = self.local_now();
        let (parse_text, s_utc, e_utc) = Self::split_span_rfc3339_utc(text);
        let Some(raw) = to_end(&grammar().all, &parse_text, local_now)? else {
            return Ok(Self::error(node, text, spec));
        };
        if let Some(unit) = time_on_period(&raw) {
            return Ok(Self::time_on_period_error(text, spec, unit, node.startchar(), node.endchar()));
        }
        let mut start_tz = if s_utc { TzSel::Utc } else { TzSel::Local };
        let mut end_tz = if e_utc { TzSel::Utc } else { TzSel::Local };
        let mut start_exact = is_exact_text(&dv_start(&raw));
        let mut end_exact = is_exact_text(&dv_end(&raw));
        let result = disambiguated(&raw, local_now)?;
        let (lo_naive, hi_naive, incl_lo, incl_hi);
        match &result {
            DV::S(s) if s.start != s.end => {
                if s.bounds_swapped {
                    std::mem::swap(&mut start_exact, &mut end_exact);
                    std::mem::swap(&mut start_tz, &mut end_tz);
                }
                let _ = start_exact;
                lo_naive = dl_dt(&s.start)?;
                let resolved_hi = dl_dt(&s.end)?;
                incl_lo = true;
                if end_exact {
                    hi_naive = resolved_hi;
                    incl_hi = true;
                } else {
                    hi_naive = add_delta(resolved_hi, Duration::microseconds(1))?;
                    incl_hi = false;
                }
            }
            DV::S(s) => {
                lo_naive = dl_dt(&s.start)?;
                hi_naive = lo_naive;
                incl_lo = true;
                incl_hi = true;
            }
            DV::Dt(d) => {
                lo_naive = *d;
                hi_naive = *d;
                incl_lo = true;
                incl_hi = true;
            }
            DV::A(_) => return Err(DErr::Value),
        }
        let lo = self.to_utc(lo_naive, spec.date_only, false, start_tz)?;
        let hi = self.to_utc(hi_naive, spec.date_only, !incl_hi, end_tz)?;
        let boost = match node {
            SNode::Text { boost, .. } => *boost,
            _ => 1.0,
        };
        Ok(SNode::DateRange {
            fieldname: spec.name.clone(),
            lo: Some(lo),
            hi: Some(hi),
            incl_lo,
            incl_hi,
            boost,
            sc: None,
            ec: None,
        })
    }

    /// `range_to_node`
    fn range_to_node(&self, node: &SNode, spec: &FieldSpec) -> SNode {
        match self.range_to_node_inner(node, spec) {
            Ok(n) => n,
            Err(_) => {
                let (s, e) = match node {
                    SNode::Range { start, end, .. } => (start.clone(), end.clone()),
                    _ => (None, None),
                };
                let t = e.filter(|x| !x.is_empty()).or(s.filter(|x| !x.is_empty())).unwrap_or_default();
                Self::error(node, &t, spec)
            }
        }
    }

    fn range_to_node_inner(&self, node: &SNode, spec: &FieldSpec) -> DResult<SNode> {
        let SNode::Range { start, end, startexcl, endexcl, .. } = node else {
            return Err(DErr::Value);
        };
        let local_now = self.local_now();
        let g = grammar();
        let mut raw_start: Option<DV> = None;
        let mut raw_end: Option<DV> = None;
        let mut start_tz = TzSel::Local;
        let mut end_tz = TzSel::Local;
        let nstart = start.clone().filter(|s| !s.is_empty());
        let nend = end.clone().filter(|s| !s.is_empty());
        if let Some(st) = &nstart {
            let (t, u) = Self::split_rfc3339_utc(st);
            if u {
                start_tz = TzSel::Utc;
            }
            match to_end(&g.all, &t, local_now) {
                Err(_) | Ok(None) => return Ok(Self::error(node, st, spec)),
                Ok(Some(r)) => {
                    if let Some(unit) = time_on_period(&r) {
                        return Ok(Self::time_on_period_error(st, spec, unit, node.startchar(), node.endchar()));
                    }
                    raw_start = Some(r);
                }
            }
        }
        if let Some(en) = &nend {
            let (t, u) = Self::split_rfc3339_utc(en);
            if u {
                end_tz = TzSel::Utc;
            }
            match to_end(&g.all, &t, local_now) {
                Err(_) | Ok(None) => return Ok(Self::error(node, en, spec)),
                Ok(Some(r)) => {
                    if let Some(unit) = time_on_period(&r) {
                        return Ok(Self::time_on_period_error(en, spec, unit, node.startchar(), node.endchar()));
                    }
                    raw_end = Some(r);
                }
            }
        }
        let exact = |r: &Option<DV>| match r {
            Some(DV::Dt(_)) => true,
            Some(DV::A(a)) => !is_ambiguous(&DL::A(*a)),
            _ => false,
        };
        let mut start_exact = exact(&raw_start);
        let mut end_exact = exact(&raw_end);
        let mut lo_naive: Option<NaiveDateTime> = None;
        let mut hi_naive: Option<NaiveDateTime> = None;
        let mut incl_lo = !*startexcl;
        let mut incl_hi = !*endexcl;
        let s_text = start.clone().unwrap_or_default();
        let e_text = end.clone().unwrap_or_default();

        let as_dl = |r: &Option<DV>| match r {
            Some(DV::Dt(d)) => Some(DL::Dt(*d)),
            Some(DV::A(a)) => Some(DL::A(*a)),
            _ => None,
        };
        let can_combine = matches!((as_dl(&raw_start), as_dl(&raw_end)), (Some(_), Some(_)));
        if let (true, Some(a), Some(b)) = (can_combine, as_dl(&raw_start), as_dl(&raw_end)) {
            match Span::new(a, b).disambiguated(local_now) {
                Ok(ts) => {
                    lo_naive = Some(dl_dt(&ts.start)?);
                    hi_naive = Some(dl_dt(&ts.end)?);
                    if ts.bounds_swapped {
                        std::mem::swap(&mut start_tz, &mut end_tz);
                        std::mem::swap(&mut start_exact, &mut end_exact);
                    }
                }
                Err(_) => {
                    if let Some(rs) = &raw_start {
                        if !matches!(rs, DV::Dt(_)) && disambiguated(rs, local_now).is_err() {
                            return Ok(Self::error(node, &s_text, spec));
                        }
                    }
                    return Ok(Self::error(node, &e_text, spec));
                }
            }
        } else {
            if let Some(rs) = &raw_start {
                let sd = disambiguated(rs, local_now)?;
                lo_naive = Some(dl_dt(&dv_start(&sd))?);
            }
            if let Some(re) = &raw_end {
                let ed = disambiguated(re, local_now)?;
                hi_naive = Some(dl_dt(&dv_end(&ed))?);
            }
        }

        let individual = |raw: &Option<DV>| -> DResult<DV> {
            match raw {
                Some(r) => disambiguated(r, local_now),
                None => Err(DErr::Value),
            }
        };
        let date_only = spec.date_only;
        let fails_as_lo = |raw: &Option<DV>| -> bool {
            individual(raw)
                .and_then(|p| dl_dt(&dv_start(&p)))
                .and_then(|lo| self.to_utc(lo, date_only, false, TzSel::Local))
                .is_err()
        };
        let incl_hi_typed = incl_hi;
        let fails_as_hi = |raw: &Option<DV>| -> bool {
            individual(raw)
                .and_then(|p| match p {
                    DV::S(s) => dl_dt(&s.end)
                        .and_then(|e| add_delta(e, Duration::microseconds(1)))
                        .and_then(|e| self.to_utc(e, date_only, true, TzSel::Local)),
                    DV::Dt(d) => self.to_utc(d, date_only, !incl_hi_typed, TzSel::Local),
                    DV::A(_) => Err(DErr::Value),
                })
                .is_err()
        };
        let blame = |side_lo: bool, default: &str| -> String {
            if !can_combine {
                return default.to_string();
            }
            if side_lo {
                if fails_as_lo(&raw_end) && !fails_as_lo(&raw_start) {
                    return e_text.clone();
                }
            } else if fails_as_hi(&raw_start) && !fails_as_hi(&raw_end) {
                return s_text.clone();
            }
            default.to_string()
        };

        if let (Some(h), false) = (hi_naive, end_exact) {
            match add_delta(h, Duration::microseconds(1)) {
                Ok(v) => hi_naive = Some(v),
                Err(_) => return Ok(Self::error(node, &blame(false, &e_text), spec)),
            }
            incl_hi = false;
        }
        if lo_naive.is_some() && !start_exact {
            incl_lo = true;
        }
        let lo = match lo_naive.map(|l| self.to_utc(l, spec.date_only, false, start_tz)).transpose() {
            Ok(v) => v,
            Err(_) => return Ok(Self::error(node, &blame(true, &s_text), spec)),
        };
        let hi = match hi_naive.map(|h| self.to_utc(h, spec.date_only, !incl_hi, end_tz)).transpose() {
            Ok(v) => v,
            Err(_) => return Ok(Self::error(node, &blame(false, &e_text), spec)),
        };
        if lo.is_none() {
            incl_lo = true;
        }
        if hi.is_none() {
            incl_hi = true;
        }
        Ok(SNode::DateRange { fieldname: spec.name.clone(), lo, hi, incl_lo, incl_hi, boost: 1.0, sc: None, ec: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Byte-slicing regression (same class as Review 2 M4): Kelvin sign shrinks and U+0130 grows
    /// when lowercased, so equal total lengths do not mean equal offsets.
    #[test]
    fn find_to_split_non_ascii_no_panic() {
        assert_eq!(
            DateParserPlugin::find_to_split("\u{212A}\u{e9}\u{e9} to \u{130}\u{130}"),
            Some(("\u{212A}\u{e9}\u{e9}".into(), "\u{130}\u{130}".into()))
        );
        assert_eq!(DateParserPlugin::find_to_split("2020 to 2021"), Some(("2020".into(), "2021".into())));
        assert_eq!(DateParserPlugin::find_to_split("\u{e4}to"), None);
    }

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    fn parse(text: &str) -> Option<DV> {
        let now = dt("2026-06-15 12:00:00");
        let raw = to_end(&grammar().all, text, now).unwrap()?;
        Some(disambiguated(&raw, now).unwrap())
    }

    #[test]
    fn grammar_basics() {
        let span = |a: &str, b: &str| {
            let mut e = dt(b);
            e = e.with_nanosecond(999_999_000).unwrap();
            DV::S(Span { start: DL::Dt(dt(a)), end: DL::Dt(e), bounds_swapped: false, kind: SpanKind::Plain })
        };
        assert_eq!(parse("2020"), Some(span("2020-01-01 00:00:00", "2020-12-31 23:59:59")));
        assert_eq!(parse("2005-03-04"), Some(span("2005-03-04 00:00:00", "2005-03-04 23:59:59")));
        assert_eq!(parse("2005-03"), Some(span("2005-03-01 00:00:00", "2005-03-31 23:59:59")));
        assert_eq!(parse("today"), Some(span("2026-06-15 00:00:00", "2026-06-15 23:59:59")));
        assert_eq!(parse("january"), Some(span("2026-01-01 00:00:00", "2026-01-31 23:59:59")));
        assert_eq!(parse("next monday"), Some(span("2026-06-22 00:00:00", "2026-06-22 23:59:59")));
        assert_eq!(parse("last monday"), Some(span("2026-06-08 00:00:00", "2026-06-08 23:59:59")));
        assert_eq!(parse("now"), Some(DV::Dt(dt("2026-06-15 12:00:00"))));
        assert_eq!(parse("-1 week"), Some(DV::Dt(dt("2026-06-08 12:00:00"))));
        assert_eq!(parse("now-7d"), Some(DV::Dt(dt("2026-06-08 12:00:00"))));
        assert_eq!(parse("2005 to 2009"), Some(span("2005-01-01 00:00:00", "2009-12-31 23:59:59")));
        assert!(parse("202023").is_none());
        assert!(parse("notadate").is_none());
        assert!(parse("2005-03-").is_none());
        // 2026-08 15:00 is a month plus a time, not 15 August.
        let now = dt("2026-06-15 12:00:00");
        let raw = to_end(&grammar().all, "2026-08 15:00", now).unwrap().unwrap();
        assert_eq!(time_on_period(&raw), Some("month"));
    }

    #[test]
    fn previous_week_is_a_calendar_period() {
        let now = dt("2026-06-17 12:00:00");
        let raw = to_end(&grammar().all, "previous week", now).unwrap().unwrap();
        let DV::S(s) = raw else { panic!() };
        assert_eq!(s.kind, SpanKind::CalendarPeriod("week"));
        assert_eq!(s.start, DL::Dt(dt("2026-06-08 00:00:00")));
        let raw = to_end(&grammar().all, "previous week 3pm", now).unwrap().unwrap();
        assert_eq!(time_on_period(&raw), Some("week"));
    }
}
