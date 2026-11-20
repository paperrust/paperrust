// upstream: dateparser 1.4.3 parser.py (tokenizer, _parser, _time_parser, resolve_date_order),
//           utils/strptime.py (strptime with the English-patched CPython _strptime),
//           CPython 3.12 Lib/_strptime.py (directive regexes, %y pivot, unconverted data)
//
// Only the absolute-time parser is ported (settings used by paperless: DATE_ORDER given
// explicitly, PREFER_DATES_FROM="current_period", PREFER_MONTH_OF_YEAR="current",
// STRICT_PARSING/REQUIRE_PARTS unset, RETURN_TIME_AS_PERIOD False). Branches that only
// act for PREFER_DATES_FROM past/future are therefore omitted (see contract).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreferDayOfMonth {
    First,
    Current,
    Last,
}

#[derive(Debug)]
pub(crate) struct ParseError;

type PResult<T> = Result<T, ParseError>;

// ---------------------------------------------------------------------------
// strptime (CPython _strptime subset, English names as patched by dateparser)
// ---------------------------------------------------------------------------

const MONTHS: [&str; 12] = [
    "january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november",
    "december",
];
const MONTH_ABBR: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
const DAYS: [&str; 7] = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];
const DAY_ABBR: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

fn seq_to_re(items: &[&str], name: &str) -> String {
    // _strptime.__seqToRE: sort by length descending
    let mut v: Vec<&str> = items.to_vec();
    v.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let body = v.iter().map(|s| regex::escape(s)).collect::<Vec<_>>().join("|");
    format!("(?P<{name}>{body})")
}

fn directive_re(d: char) -> String {
    match d {
        'd' => r"(?P<d>3[01]|[12]\d|0[1-9]|[1-9]| [1-9])".into(),
        'm' => r"(?P<m>1[0-2]|0[1-9]|[1-9])".into(),
        'y' => r"(?P<y>\d\d)".into(),
        'Y' => r"(?P<Y>\d\d\d\d)".into(),
        'H' => r"(?P<H>2[0-3]|[0-1]\d|\d)".into(),
        'I' => r"(?P<I>1[0-2]|0[1-9]|[1-9])".into(),
        'M' => r"(?P<M>[0-5]\d|\d)".into(),
        'S' => r"(?P<S>6[0-1]|[0-5]\d|\d)".into(),
        'f' => r"(?P<f>[0-9]{1,6})".into(),
        'p' => r"(?P<p>am|pm)".into(),
        'B' => seq_to_re(&MONTHS, "B"),
        'b' => seq_to_re(&MONTH_ABBR, "b"),
        'A' => seq_to_re(&DAYS, "A"),
        'a' => seq_to_re(&DAY_ABBR, "a"),
        _ => regex::escape(&d.to_string()),
    }
}

fn format_regex(fmt: &str) -> Regex {
    static CACHE: LazyLock<Mutex<HashMap<String, Regex>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    if let Ok(c) = CACHE.lock() {
        if let Some(r) = c.get(fmt) {
            return r.clone();
        }
    }
    let mut out = String::from("^");
    let chars: Vec<char> = fmt.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '%' && i + 1 < chars.len() {
            out.push_str(&directive_re(chars[i + 1]));
            i += 2;
        } else if c.is_whitespace() {
            out.push_str(r"\s+");
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
        } else {
            out.push_str(&regex::escape(&c.to_string()));
            i += 1;
        }
    }
    let re = regex::RegexBuilder::new(&out).case_insensitive(true).build().unwrap_or_else(|_| {
        // all formats are static and valid; fall back to a never-matching regex
        Regex::new(r"[^\s\S]").unwrap_or_else(|_| unreachable!())
    });
    if let Ok(mut c) = CACHE.lock() {
        c.insert(fmt.to_string(), re.clone());
    }
    re
}

/// `dateparser.utils.strptime.strptime(date_string, format)` -> naive datetime.
pub(crate) fn strptime(date_string: &str, fmt: &str, today_year: i32) -> PResult<NaiveDateTime> {
    // _prepare_format: day of month without year -> prepend the current year
    let has_year = fmt.contains("%Y") || fmt.contains("%y");
    let (s, f) = if fmt.contains("%d") && !has_year {
        (format!("{today_year} {date_string}"), format!("%Y {fmt}"))
    } else {
        (date_string.to_string(), fmt.to_string())
    };
    let re = format_regex(&f);
    let caps = re.captures(&s).ok_or(ParseError)?;
    if caps.get(0).map_or(0, |m| m.end()) != s.len() {
        return Err(ParseError); // unconverted data remains
    }
    let g = |n: &str| caps.name(n).map(|m| m.as_str());
    let mut year = 1900i32;
    let mut month = 1u32;
    let mut day = 1u32;
    let (mut hour, mut minute, mut second, mut micro) = (0u32, 0u32, 0u32, 0u32);
    if let Some(y) = g("y") {
        let v: i32 = y.parse().map_err(|_| ParseError)?;
        year = if v <= 68 { v + 2000 } else { v + 1900 };
    }
    if let Some(y) = g("Y") {
        year = y.parse().map_err(|_| ParseError)?;
    }
    if let Some(m) = g("m") {
        month = m.parse().map_err(|_| ParseError)?;
    }
    if let Some(b) = g("B") {
        month = MONTHS.iter().position(|x| x.eq_ignore_ascii_case(b)).ok_or(ParseError)? as u32 + 1;
    }
    if let Some(b) = g("b") {
        month = MONTH_ABBR.iter().position(|x| x.eq_ignore_ascii_case(b)).ok_or(ParseError)? as u32 + 1;
    }
    if let Some(d) = g("d") {
        day = d.trim().parse().map_err(|_| ParseError)?;
    }
    if let Some(h) = g("H") {
        hour = h.parse().map_err(|_| ParseError)?;
    }
    if let Some(h) = g("I") {
        hour = h.parse().map_err(|_| ParseError)?;
        let ampm = g("p").unwrap_or("").to_lowercase();
        if ampm.is_empty() || ampm == "am" {
            if hour == 12 {
                hour = 0;
            }
        } else if ampm == "pm" && hour != 12 {
            hour += 12;
        }
    }
    if let Some(m) = g("M") {
        minute = m.parse().map_err(|_| ParseError)?;
    }
    if let Some(sx) = g("S") {
        second = sx.parse().map_err(|_| ParseError)?;
    }
    if let Some(f) = g("f") {
        let padded = format!("{f:0<6}");
        micro = padded.parse().map_err(|_| ParseError)?;
    }
    if year < 1 {
        return Err(ParseError);
    }
    let date = NaiveDate::from_ymd_opt(year, month, day).ok_or(ParseError)?;
    let time = NaiveTime::from_hms_micro_opt(hour, minute, second, micro).ok_or(ParseError)?;
    Ok(NaiveDateTime::new(date, time))
}

/// `_time_parser.__call__`
pub(crate) fn time_parser(timestring: &str) -> PResult<NaiveTime> {
    const DIRECTIVES: [&str; 8] =
        ["%H:%M:%S", "%I:%M:%S %p", "%H:%M", "%I:%M %p", "%I %p", "%H:%M:%S.%f", "%I:%M:%S.%f %p", "%H:%M %p"];
    let t = timestring.trim();
    for d in DIRECTIVES {
        if let Ok(dt) = strptime(t, d, 1900) {
            return Ok(dt.time());
        }
    }
    Err(ParseError)
}

// ---------------------------------------------------------------------------
// tokenizer
// ---------------------------------------------------------------------------

fn char_type(c: char) -> u8 {
    if "0123456789:".contains(c) {
        0
    } else if c.is_ascii_alphabetic() {
        1
    } else {
        2
    }
}

/// `tokenizer(ds).tokenize()` -> (token, type)
pub(crate) fn tokenize(ds: &str) -> Vec<(String, u8)> {
    let mut out = Vec::new();
    let mut token = String::new();
    for c in ds.chars() {
        if let Some(last) = token.chars().last() {
            let t = char_type(last);
            let switch = match t {
                0 => char_type(c) != 0,
                1 => char_type(c) != 1,
                _ => char_type(c) != 2,
            };
            if switch {
                out.push((std::mem::take(&mut token), t));
            }
            token.push(c);
        } else {
            token.push(c);
        }
    }
    if let Some(last) = token.chars().last() {
        out.push((token.clone(), char_type(last)));
    }
    out
}

// ---------------------------------------------------------------------------
// _parser
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Comp {
    Day,
    Month,
    Year,
}

fn order_components(order: &str) -> [Comp; 3] {
    match order {
        "DMY" => [Comp::Day, Comp::Month, Comp::Year],
        "DYM" => [Comp::Day, Comp::Year, Comp::Month],
        "MDY" => [Comp::Month, Comp::Day, Comp::Year],
        "MYD" => [Comp::Month, Comp::Year, Comp::Day],
        "YDM" => [Comp::Year, Comp::Day, Comp::Month],
        _ => [Comp::Year, Comp::Month, Comp::Day], // YMD
    }
}

fn directives(c: Comp) -> &'static [&'static str] {
    match c {
        Comp::Day => &["%d"],
        Comp::Month => &["%m"],
        Comp::Year => &["%y", "%Y"],
    }
}

pub(crate) struct ParseSettings {
    pub date_order: String,
    pub prefer_day: PreferDayOfMonth,
    /// `datetime.now(tz=utc).replace(tzinfo=None)`
    pub now_utc: NaiveDateTime,
    /// `datetime.today().year` (local) for strptime's %d handling
    pub today_year: i32,
}

#[derive(Default)]
struct State {
    day: Option<u32>,
    month: Option<u32>,
    year: Option<i32>,
    weekday: bool,
    time_token: Option<String>,
    token_day: Option<String>,
    token_month: Option<String>,
    token_year: Option<String>,
    token_weekday: Option<String>,
    // stored (token, type) for re-parse checks
    tok_tuple: HashMap<&'static str, (String, u8)>,
    unset_tokens: Vec<(String, u8)>,
    auto_order: Vec<&'static str>,
}

fn cname(c: Comp) -> &'static str {
    match c {
        Comp::Day => "day",
        Comp::Month => "month",
        Comp::Year => "year",
    }
}

impl State {
    fn get(&self, c: Comp) -> bool {
        match c {
            Comp::Day => self.day.is_some_and(|v| v != 0),
            Comp::Month => self.month.is_some_and(|v| v != 0),
            Comp::Year => self.year.is_some_and(|v| v != 0),
        }
    }
    fn set(&mut self, c: Comp, dt: &NaiveDateTime) {
        match c {
            Comp::Day => self.day = Some(dt.day()),
            Comp::Month => self.month = Some(dt.month()),
            Comp::Year => self.year = Some(dt.year()),
        }
    }
    fn set_token(&mut self, c: Comp, token: &str, ty: u8) {
        let t = Some(token.to_string());
        match c {
            Comp::Day => self.token_day = t,
            Comp::Month => self.token_month = t,
            Comp::Year => self.token_year = t,
        }
        self.tok_tuple.insert(cname(c), (token.to_string(), ty));
    }
}

fn parse_number(st: &mut State, s: &ParseSettings, token: &str, skip_component: Option<Comp>) -> PResult<Vec<Comp>> {
    let ty = 0u8;
    let order = order_components(&s.date_order);

    let try_directives = |st: &mut State, skip_directive: Option<&str>| -> PResult<Vec<Comp>> {
        for comp in order {
            if skip_component == Some(comp) {
                continue;
            }
            for &directive in directives(comp) {
                if Some(directive) == skip_directive {
                    continue;
                }
                let Ok(dobj) = strptime(token, directive, s.today_year) else { continue };
                if !st.get(comp) {
                    st.auto_order.push(cname(comp));
                    st.set_token(comp, token, ty);
                    st.set(comp, &dobj);
                    return Ok(vec![comp]);
                }
                let prev = st.tok_tuple.get(cname(comp)).cloned();
                match prev {
                    Some((prev_token, prev_type)) => {
                        if prev_type == ty && strptime(&prev_token, directive, s.today_year).is_err() {
                            st.unset_tokens.push((prev_token, prev_type));
                            st.auto_order.push(cname(comp));
                            st.set_token(comp, token, ty);
                            st.set(comp, &dobj);
                            return Ok(vec![comp]);
                        }
                    }
                    None => {
                        // `prev_token, prev_type = getattr(...)` on a str token raises ValueError
                    }
                }
            }
        }
        Err(ParseError)
    };

    let year_pos = order.iter().position(|c| *c == Comp::Year).unwrap_or(2);
    let year_passed = order[year_pos + 1..].iter().any(|c| match c {
        Comp::Day => st.day.is_some(),
        Comp::Month => st.month.is_some(),
        Comp::Year => st.year.is_some(),
    });
    if year_passed {
        if let Ok(r) = try_directives(st, Some("%y")) {
            return Ok(r);
        }
    }
    try_directives(st, None)
}

fn parse_alpha(st: &mut State, s: &ParseSettings, token: &str, skip_component: Option<&str>) -> PResult<()> {
    let ty = 1u8;
    for (component, dirs) in [("weekday", ["%A", "%a"]), ("month", ["%B", "%b"])] {
        if skip_component == Some(component) {
            continue;
        }
        for d in dirs {
            let Ok(dobj) = strptime(token, d, s.today_year) else { continue };
            if component == "weekday" {
                if !st.weekday {
                    st.weekday = true;
                    st.token_weekday = Some(token.to_string());
                    return Ok(());
                }
            } else if !st.get(Comp::Month) {
                st.token_month = Some(token.to_string());
                st.tok_tuple.insert("month", (token.to_string(), ty));
                st.month = Some(dobj.month());
                return Ok(());
            } else if let Some(idx) = st.auto_order.iter().position(|c| *c == "month") {
                let prev_value = st.month;
                st.auto_order[idx] = "day";
                st.token_day = st.token_month.clone();
                if let Some(t) = st.tok_tuple.get("month").cloned() {
                    st.tok_tuple.insert("day", t);
                }
                st.token_month = Some(token.to_string());
                st.tok_tuple.insert("month", (token.to_string(), ty));
                st.month = Some(dobj.month());
                st.day = prev_value;
                return Ok(());
            }
            // auto_order.index("month") raised -> swallowed, try next directive
        }
    }
    Err(ParseError)
}

static HOUR_MINUTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([0-9]|0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]$").unwrap());
static MICROSECOND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d{1,6}").unwrap());
static MERIDIAN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"am|pm").unwrap());

fn last_day_of_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    NaiveDate::from_ymd_opt(ny, nm, 1).and_then(|d| d.pred_opt()).map_or(28, |d| d.day())
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn closer_leap_year(y: i32) -> i32 {
    let mut next = y + 1;
    while !is_leap(next) {
        next += 1;
    }
    let mut prev = y - 1;
    while !is_leap(prev) {
        prev -= 1;
    }
    if next - y < y - prev { next } else { prev }
}

/// `_parser.parse(datestring, settings, tz, date_order)` -> (naive datetime, period)
pub(crate) fn parse_absolute(datestring: &str, s: &ParseSettings) -> PResult<(NaiveDateTime, &'static str)> {
    let raw = tokenize(datestring);
    let tokens: Vec<(String, u8)> = raw.into_iter().map(|(t, ty)| (t.trim().to_string(), ty)).collect();
    let filtered: Vec<(String, u8, usize)> =
        tokens.iter().enumerate().filter(|(_, t)| t.1 <= 1).map(|(i, t)| (t.0.clone(), t.1, i)).collect();

    let mut st = State::default();
    let mut skip_index: Vec<usize> = Vec::new();
    let mut skip_component: Option<Comp> = None;
    const SKIP_TOKENS: [&str; 4] = ["t", "year", "hour", "minute"];

    for (index, (tok, ty, original_index)) in filtered.iter().enumerate() {
        if skip_index.contains(&index) {
            continue;
        }
        let mut token = tok.clone();
        let ty = *ty;
        let original_index = *original_index;
        if SKIP_TOKENS.contains(&token.as_str()) {
            continue;
        }

        if st.time_token.is_none() {
            let mut meridian_index = index + 1;

            // hours and minutes separated by a period, e.g. 13.20
            (|| -> Option<()> {
                let is_before_period = tokens.get(original_index + 1)?.0 == ".";
                let is_after_period = original_index != 0 && tokens.get(original_index - 1)?.0 == ".";
                if is_before_period && !is_after_period {
                    let index_next = index + 1;
                    let next = filtered.get(index_next)?;
                    let next_is_last = index_next == filtered.len() - 1;
                    let after_next_is_period = if next_is_last {
                        false
                    } else {
                        tokens.get(next.2 + 1)?.0 == "."
                    };
                    if next_is_last || !after_next_is_period {
                        let new_token = format!("{}:{}", token, next.0);
                        if HOUR_MINUTE.is_match(&new_token) {
                            token = new_token;
                            skip_index.push(index + 1);
                            meridian_index += 1;
                        }
                    }
                }
                Some(())
            })();

            let microsecond: Option<String> = (|| -> Option<String> {
                let ms = MICROSECOND.find(&filtered.get(index + 1)?.0)?.as_str().to_string();
                token.find(':')?;
                let pos = tokens.iter().position(|t| t.0 == token && t.1 == 0)?;
                tokens.get(pos + 1)?.0.find('.')?;
                Some(ms)
            })();
            if microsecond.is_some() {
                meridian_index += 1;
            }
            let meridian: Option<String> =
                filtered.get(meridian_index).and_then(|t| MERIDIAN.find(&t.0)).map(|m| m.as_str().to_string());

            if token.contains(':') || meridian.is_some() || microsecond.is_some() {
                st.time_token = Some(match (&meridian, &microsecond) {
                    (Some(m), None) => {
                        skip_index.push(meridian_index);
                        format!("{token} {m}")
                    }
                    (None, Some(us)) => {
                        skip_index.push(index + 1);
                        format!("{token}.{us}")
                    }
                    (Some(m), Some(us)) => {
                        skip_index.push(index + 1);
                        skip_index.push(meridian_index);
                        format!("{token}.{us} {m}")
                    }
                    (None, None) => token.clone(),
                });
                continue;
            }
        }

        if ty == 0 {
            let comps = parse_number(&mut st, s, &token, skip_component)?;
            for c in comps {
                if token.chars().count() == 4 && c == Comp::Year {
                    skip_component = Some(Comp::Year);
                }
            }
        } else {
            parse_alpha(&mut st, s, &token, skip_component.map(cname))?;
        }
    }

    // unresolved attrs from unset tokens
    let unknown: Vec<Comp> =
        [Comp::Year, Comp::Month, Comp::Day].into_iter().filter(|c| match c {
            Comp::Day => st.day.is_none(),
            Comp::Month => st.month.is_none(),
            Comp::Year => st.year.is_none(),
        }).collect();
    for attr in unknown {
        for (tok, ty) in st.unset_tokens.clone() {
            if ty == 0 {
                let v: i64 = tok.parse().map_err(|_| ParseError)?;
                match attr {
                    Comp::Day => {
                        st.day = Some(v as u32);
                        st.token_day = Some(tok.clone());
                    }
                    Comp::Month => {
                        st.month = Some(v as u32);
                        st.token_month = Some(tok.clone());
                    }
                    Comp::Year => {
                        st.year = Some(v as i32);
                        st.token_year = Some(tok.clone());
                    }
                }
                st.tok_tuple.remove(cname(attr));
            }
        }
    }

    // _results()
    let now = s.now_utc;
    let time = match &st.time_token {
        Some(t) => Some(time_parser(t)?),
        None => None,
    };
    let mut day = st.day.filter(|d| *d != 0).unwrap_or(now.day());
    let month = st.month.filter(|m| *m != 0).unwrap_or(now.month());
    let mut year = st.year.filter(|y| *y != 0).unwrap_or(now.year());
    if !(1..=12).contains(&month) || !(1..=9999).contains(&year) {
        return Err(ParseError);
    }
    if day == 0 || day > last_day_of_month(year, month) {
        // "day is out of range for month"
        if !(st.token_day.is_some() || st.token_weekday.is_some()) {
            day = last_day_of_month(year, month);
        } else if st.token_year.is_none() && day == 29 && month == 2 && !is_leap(year) {
            year = closer_leap_year(year);
        } else {
            return Err(ParseError);
        }
    }
    let date = NaiveDate::from_ymd_opt(year, month, day).ok_or(ParseError)?;
    let mut dateobj = NaiveDateTime::new(date, time.unwrap_or(NaiveTime::MIN));

    // _correct_for_time_frame (PREFER_DATES_FROM = current_period)
    if let Some(wd) = &st.token_weekday {
        if st.token_year.is_none() && st.token_month.is_none() && st.token_day.is_none() {
            let wanted: String = wd.chars().take(3).collect::<String>().to_lowercase();
            let mut idx = dateobj.weekday().num_days_from_monday() as i64;
            let mut steps = 0i64;
            if DAY_ABBR[idx as usize] != wanted {
                while DAY_ABBR[idx.rem_euclid(7) as usize] != wanted {
                    idx -= 1;
                    steps += 1;
                    if steps > 7 {
                        break;
                    }
                }
            }
            dateobj -= chrono::Duration::days(steps);
            st.token_month = Some(dateobj.month().to_string());
        }
    }

    // _correct_for_month (PREFER_MONTH_OF_YEAR = current)
    if st.token_month.is_none() {
        dateobj = dateobj
            .with_month(now.month())
            .or_else(|| dateobj.with_month(12))
            .ok_or(ParseError)?;
    }

    // _correct_for_day
    if st.token_day.is_none() && st.token_weekday.is_none() && st.time_token.is_none() {
        let last = last_day_of_month(dateobj.year(), dateobj.month());
        let target = match s.prefer_day {
            PreferDayOfMonth::First => 1,
            PreferDayOfMonth::Last => last,
            PreferDayOfMonth::Current => now.day(),
        };
        dateobj = dateobj.with_day(target).or_else(|| dateobj.with_day(last)).ok_or(ParseError)?;
    }

    // _get_period
    let period = if time.is_some() || st.day.is_some_and(|d| d != 0) {
        "day"
    } else if st.month.is_some_and(|m| m != 0) {
        "month"
    } else if st.year.is_some_and(|y| y != 0) {
        "year"
    } else {
        "day"
    };
    let _ = dateobj.hour();
    Ok((dateobj, period))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(order: &str) -> ParseSettings {
        ParseSettings {
            date_order: order.into(),
            prefer_day: PreferDayOfMonth::First,
            now_utc: NaiveDate::from_ymd_opt(2026, 11, 7).unwrap().and_hms_opt(12, 0, 0).unwrap(),
            today_year: 2026,
        }
    }

    fn ymd(s: &str, order: &str) -> Option<(i32, u32, u32)> {
        parse_absolute(s, &settings(order)).ok().map(|(d, _)| (d.year(), d.month(), d.day()))
    }

    #[test]
    fn strptime_semantics() {
        assert!(strptime("123", "%d", 2026).is_err());
        assert!(strptime("0", "%d", 2026).is_err());
        assert_eq!(strptime("24", "%y", 2026).unwrap().year(), 2024);
        assert_eq!(strptime("69", "%y", 2026).unwrap().year(), 1969);
        assert!(strptime("2024", "%y", 2026).is_err());
        assert!(strptime("0000", "%Y", 2026).is_err());
        assert_eq!(strptime("may", "%B", 2026).unwrap().month(), 5);
        assert_eq!(time_parser("10:30 pm").unwrap(), NaiveTime::from_hms_opt(22, 30, 0).unwrap());
    }

    #[test]
    fn absolute() {
        assert_eq!(ymd("15/01/2024", "DMY"), Some((2024, 1, 15)));
        assert_eq!(ymd("01/15/2024", "MDY"), Some((2024, 1, 15)));
        assert_eq!(ymd("2024/01/15", "YMD"), Some((2024, 1, 15)));
        assert_eq!(ymd("january 15, 2024", "DMY"), Some((2024, 1, 15)));
        assert_eq!(ymd("15 january 2024", "DMY"), Some((2024, 1, 15)));
        assert_eq!(ymd("15.01.2024", "DMY"), Some((2024, 1, 15)));
        assert_eq!(ymd("2024-01-15", "YMD"), Some((2024, 1, 15)));
        assert_eq!(ymd("32/13/2024", "DMY"), None);
        assert_eq!(ymd("99/99/9999", "DMY"), None);
        assert_eq!(ymd("31/02/2024", "DMY"), None);
        assert_eq!(ymd("march 2024", "DMY"), Some((2024, 3, 1)));
        // ambiguity swap: day and month both <= 12
        assert_eq!(ymd("01/02/2023", "DMY"), Some((2023, 2, 1)));
        assert_eq!(ymd("13/02/2023", "MDY"), Some((2023, 2, 13)));
        assert_eq!(ymd("11.01.10", "DMY"), Some((2010, 1, 11)));
    }
}
