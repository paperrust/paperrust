//! Excel number formats for cell text (XLSX and XLS) – like Tika, which outputs cells via Apache
//! POI `DataFormatter` (en-US locale) as displayed text (gap E13).
//!
//! upstream: Tika `XSSFExcelExtractorDecorator`/`ExcelExtractor` → POI `DataFormatter`
//! (`BuiltinFormats`, `DateUtil.isADateFormat`, `ExcelStyleDateFormatter`); "General" like
//! `TikaExcelGeneralFormat` (full precision, whole numbers without decimals).
//!
//! Covered: all built-in formats 0–49, custom date/time formats (y, m, d, h, s, AM/PM,
//! `[h]`, fractions of seconds, month/day names in English), simple number formats (0, #,
//! thousands comma, decimals, percent, sections for negative/zero, text in
//! quotation marks, `\x`, `_x`, `[$€-407]`). Scientific and fractions → "General".

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, Timelike};

/// POI `BuiltinFormats` (en-US).
pub fn builtin(id: u32) -> Option<&'static str> {
    Some(match id {
        0 => "General",
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        5 => "\"$\"#,##0_);(\"$\"#,##0)",
        6 => "\"$\"#,##0_);[Red](\"$\"#,##0)",
        7 => "\"$\"#,##0.00_);(\"$\"#,##0.00)",
        8 => "\"$\"#,##0.00_);[Red](\"$\"#,##0.00)",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        13 => "# ??/??",
        14 => "m/d/yy",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "m/d/yy h:mm",
        37 => "#,##0_);(#,##0)",
        38 => "#,##0_);[Red](#,##0)",
        39 => "#,##0.00_);(#,##0.00)",
        40 => "#,##0.00_);[Red](#,##0.00)",
        41 => "_(* #,##0_);_(* (#,##0);_(* \"-\"_);_(@_)",
        42 => "_(\"$\"* #,##0_);_(\"$\"* (#,##0);_(\"$\"* \"-\"_);_(@_)",
        43 => "_(* #,##0.00_);_(* (#,##0.00);_(* \"-\"??_);_(@_)",
        44 => "_(\"$\"* #,##0.00_);_(\"$\"* (#,##0.00);_(\"$\"* \"-\"??_);_(@_)",
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mm:ss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => return None,
    })
}

/// Format code for a format number: custom formats (`numFmts`/FORMAT) before the built-in ones.
pub fn resolve(id: u32, custom: &[(u32, String)]) -> Option<&str> {
    custom.iter().find(|(i, _)| *i == id).map(|(_, c)| c.as_str()).or_else(|| builtin(id))
}

/// Sections (`;`) outside quotation marks/escapes.
fn sections(code: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut quoted, mut esc) = (0usize, false, false);
    for (i, c) in code.char_indices() {
        if esc {
            esc = false;
            continue;
        }
        match c {
            '\\' if !quoted => esc = true,
            '"' => quoted = !quoted,
            ';' if !quoted => {
                out.push(&code[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&code[start..]);
    out
}

/// POI `DateUtil.isADateFormat`: built-in date formats 14–22, 45–47 or a code that contains
/// y, m, d, h or s outside quotation marks/brackets.
pub fn is_date_format(id: u32, code: &str) -> bool {
    if matches!(id, 14..=22 | 45..=47) {
        return true;
    }
    let first = sections(code).first().copied().unwrap_or("");
    let mut quoted = false;
    let mut bracket = false;
    let mut esc = false;
    let mut found = false;
    for c in first.chars() {
        if esc {
            esc = false;
            continue;
        }
        match c {
            '\\' if !quoted => esc = true,
            '"' => quoted = !quoted,
            '[' if !quoted => bracket = true,
            ']' if !quoted => bracket = false,
            _ if quoted || bracket => {}
            'y' | 'Y' | 'm' | 'M' | 'd' | 'D' | 'h' | 'H' | 's' | 'S' => found = true,
            // Number placeholders → not a date (e.g. "0.00 \"m\"" is already excluded)
            '0' | '#' | '?' | '@' => return false,
            _ => {}
        }
    }
    // `[h]`, `[mm]`, `[ss]` (elapsed time) count as well
    found || first.contains("[h") || first.contains("[H") || first.contains("[m]") || first.contains("[s]")
}

/// Excel serial number → date/time (`date1904`: 1904 date system). Like POI rounded to whole
/// milliseconds, then to whole seconds.
pub fn serial_to_datetime(v: f64, date1904: bool) -> Option<NaiveDateTime> {
    if !v.is_finite() || v < 0.0 || v > 2_958_466.0 {
        return None;
    }
    let days = v.floor();
    let base = if date1904 {
        NaiveDate::from_ymd_opt(1904, 1, 1)?
    } else if days < 61.0 {
        // Excel counts 29.02.1900; before that the base is one day later.
        NaiveDate::from_ymd_opt(1899, 12, 31)?
    } else {
        NaiveDate::from_ymd_opt(1899, 12, 30)?
    };
    let ms = ((v - days) * 86_400_000.0).round() as i64;
    let secs = (ms + 500) / 1000;
    base.and_hms_opt(0, 0, 0)?.checked_add_signed(Duration::days(days as i64) + Duration::seconds(secs))
}

const MONTHS: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const DAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Lit(String),
    Year(usize),
    MonthOrMin(usize),
    Day(usize),
    Hour(usize),
    Sec(usize),
    Frac(usize),
    AmPm(String),
    Elapsed(char, usize),
}

fn tokenize_date(code: &str) -> Vec<Tok> {
    let cs: Vec<char> = code.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    let run = |i: usize, c: char| cs[i..].iter().take_while(|x| x.eq_ignore_ascii_case(&c)).count();
    while i < cs.len() {
        let c = cs[i];
        match c {
            '"' => {
                let end = cs[i + 1..].iter().position(|x| *x == '"').map_or(cs.len(), |p| i + 1 + p);
                out.push(Tok::Lit(cs[i + 1..end].iter().collect()));
                i = end + 1;
            }
            '\\' => {
                if let Some(n) = cs.get(i + 1) {
                    out.push(Tok::Lit(n.to_string()));
                }
                i += 2;
            }
            '_' => {
                out.push(Tok::Lit(" ".into()));
                i += 2;
            }
            '*' => i += 2,
            '[' => {
                let end = cs[i..].iter().position(|x| *x == ']').map_or(cs.len(), |p| i + p);
                let inner: String = cs[i + 1..end].iter().collect();
                let l = inner.to_ascii_lowercase();
                if !l.is_empty() && l.chars().all(|x| x == 'h') {
                    out.push(Tok::Elapsed('h', l.len()));
                } else if !l.is_empty() && l.chars().all(|x| x == 'm') {
                    out.push(Tok::Elapsed('m', l.len()));
                } else if !l.is_empty() && l.chars().all(|x| x == 's') {
                    out.push(Tok::Elapsed('s', l.len()));
                }
                // Colors, conditions, `[$-407]` are dropped
                i = end + 1;
            }
            'y' | 'Y' => {
                let n = run(i, 'y');
                out.push(Tok::Year(n));
                i += n;
            }
            'm' | 'M' => {
                let n = run(i, 'm');
                out.push(Tok::MonthOrMin(n));
                i += n;
            }
            'd' | 'D' => {
                let n = run(i, 'd');
                out.push(Tok::Day(n));
                i += n;
            }
            'h' | 'H' => {
                let n = run(i, 'h');
                out.push(Tok::Hour(n));
                i += n;
            }
            's' | 'S' => {
                let n = run(i, 's');
                out.push(Tok::Sec(n));
                i += n;
            }
            '.' if matches!(out.last(), Some(Tok::Sec(_) | Tok::Elapsed('s', _))) && cs.get(i + 1) == Some(&'0') => {
                let n = cs[i + 1..].iter().take_while(|x| **x == '0').count();
                out.push(Tok::Frac(n));
                i += 1 + n;
            }
            'a' | 'A' => {
                let rest: String = cs[i..].iter().take(5).collect();
                if rest.eq_ignore_ascii_case("am/pm") {
                    out.push(Tok::AmPm(rest));
                    i += 5;
                } else if cs[i..].iter().take(3).collect::<String>().eq_ignore_ascii_case("a/p") {
                    out.push(Tok::AmPm(cs[i..i + 3].iter().collect()));
                    i += 3;
                } else {
                    out.push(Tok::Lit(c.to_string()));
                    i += 1;
                }
            }
            _ => {
                out.push(Tok::Lit(c.to_string()));
                i += 1;
            }
        }
    }
    out
}

/// Date/time according to an Excel date format (first section).
pub fn format_date(v: f64, code: &str, date1904: bool) -> Option<String> {
    let dt = serial_to_datetime(v, date1904)?;
    let first = sections(code).first().copied().unwrap_or(code);
    let toks = tokenize_date(first);
    let twelve = toks.iter().any(|t| matches!(t, Tok::AmPm(_)));
    let mut out = String::new();
    for (k, t) in toks.iter().enumerate() {
        match t {
            Tok::Lit(s) => out.push_str(s),
            Tok::Year(n) => {
                if *n <= 2 {
                    out.push_str(&format!("{:02}", dt.year().rem_euclid(100)));
                } else {
                    out.push_str(&format!("{:04}", dt.year()));
                }
            }
            Tok::MonthOrMin(n) => {
                // Minute if directly (without a date part in between) after an hour or before seconds
                let prev_h = toks[..k].iter().rev().find(|t| !matches!(t, Tok::Lit(_))).is_some_and(|t| matches!(t, Tok::Hour(_) | Tok::Elapsed('h', _)));
                let next_s = toks[k + 1..].iter().find(|t| !matches!(t, Tok::Lit(_))).is_some_and(|t| matches!(t, Tok::Sec(_) | Tok::Elapsed('s', _)));
                if *n <= 2 && (prev_h || next_s) {
                    if *n == 2 {
                        out.push_str(&format!("{:02}", dt.minute()));
                    } else {
                        out.push_str(&dt.minute().to_string());
                    }
                } else {
                    let m = dt.month0() as usize;
                    match n {
                        1 => out.push_str(&(m + 1).to_string()),
                        2 => out.push_str(&format!("{:02}", m + 1)),
                        3 => out.push_str(&MONTHS[m][..3]),
                        5 => out.push_str(&MONTHS[m][..1]),
                        _ => out.push_str(MONTHS[m]),
                    }
                }
            }
            Tok::Day(n) => {
                let wd = dt.weekday().num_days_from_monday() as usize;
                match n {
                    1 => out.push_str(&dt.day().to_string()),
                    2 => out.push_str(&format!("{:02}", dt.day())),
                    3 => out.push_str(&DAYS[wd][..3]),
                    _ => out.push_str(DAYS[wd]),
                }
            }
            Tok::Hour(n) => {
                let h = if twelve {
                    match dt.hour() % 12 {
                        0 => 12,
                        h => h,
                    }
                } else {
                    dt.hour()
                };
                if *n >= 2 {
                    out.push_str(&format!("{h:02}"));
                } else {
                    out.push_str(&h.to_string());
                }
            }
            Tok::Sec(n) => {
                if *n >= 2 {
                    out.push_str(&format!("{:02}", dt.second()));
                } else {
                    out.push_str(&dt.second().to_string());
                }
            }
            Tok::Frac(n) => {
                let frac = (v.fract() * 86_400.0).fract();
                let digits = format!("{:.*}", *n, frac);
                out.push('.');
                out.push_str(digits.split('.').nth(1).unwrap_or(""));
            }
            Tok::AmPm(s) => {
                let pm = dt.hour() >= 12;
                let lower = s.chars().next().is_some_and(char::is_lowercase);
                let txt = if s.len() == 3 { if pm { "P" } else { "A" } } else if pm { "PM" } else { "AM" };
                out.push_str(&if lower { txt.to_lowercase() } else { txt.to_owned() });
            }
            Tok::Elapsed(u, n) => {
                let total_secs = (v * 86_400.0).round() as i64;
                let val = match u {
                    'h' => total_secs / 3600,
                    'm' => total_secs / 60,
                    _ => total_secs,
                };
                out.push_str(&format!("{val:0width$}", width = *n));
            }
        }
    }
    Some(out)
}

/// Number like Tika "General": whole numbers without decimals, otherwise shortest form.
pub fn general(v: f64) -> String {
    if !v.is_finite() {
        return v.to_string();
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    format!("{v}")
}

/// Simple number format (one section, without sign logic). `None` → not supported.
fn format_plain(v: f64, sec: &str) -> Option<String> {
    let cs: Vec<char> = sec.chars().collect();
    let (mut prefix, mut suffix) = (String::new(), String::new());
    let mut pattern = String::new();
    let mut percent = false;
    let mut i = 0usize;
    let mut seen_digit = false;
    let mut in_number = false;
    while i < cs.len() {
        let c = cs[i];
        let lit: Option<String> = match c {
            '"' => {
                let end = cs[i + 1..].iter().position(|x| *x == '"').map_or(cs.len(), |p| i + 1 + p);
                let s: String = cs[i + 1..end].iter().collect();
                i = end + 1;
                Some(s)
            }
            '\\' => {
                let s = cs.get(i + 1).map(char::to_string).unwrap_or_default();
                i += 2;
                Some(s)
            }
            '_' => {
                i += 2;
                Some(" ".into())
            }
            '*' => {
                i += 2;
                Some(String::new())
            }
            '[' => {
                let end = cs[i..].iter().position(|x| *x == ']').map_or(cs.len(), |p| i + p);
                let inner: String = cs[i + 1..end].iter().collect();
                i = end + 1;
                // `[$€-407]` → currency symbol, otherwise (color/condition) nothing
                Some(inner.strip_prefix('$').map(|s| s.split('-').next().unwrap_or("").to_owned()).unwrap_or_default())
            }
            '0' | '#' | '?' | '.' => {
                seen_digit |= c != '.';
                in_number = true;
                pattern.push(c);
                i += 1;
                None
            }
            ',' if seen_digit => {
                in_number = true;
                pattern.push(c);
                i += 1;
                None
            }
            '%' => {
                percent = true;
                i += 1;
                Some("%".into())
            }
            'E' | 'e' | '/' | '@' => return None,
            _ => {
                i += 1;
                Some(c.to_string())
            }
        };
        if let Some(s) = lit {
            if in_number || seen_digit {
                suffix.push_str(&s);
            } else {
                prefix.push_str(&s);
            }
        }
    }
    if pattern.is_empty() {
        return Some(format!("{prefix}{suffix}"));
    }
    // Comma directly before the decimal point or at the end scales by 1000
    let mut v = if percent { v * 100.0 } else { v };
    let mut pat = pattern.clone();
    while pat.ends_with(',') {
        pat.pop();
        v /= 1000.0;
    }
    let (int_pat, frac_pat) = match pat.split_once('.') {
        Some((a, b)) => (a.to_owned(), b.replace(',', "")),
        None => (pat.clone(), String::new()),
    };
    let grouping = int_pat.contains(',');
    let min_int = int_pat.chars().filter(|c| *c == '0').count();
    let decimals = frac_pat.chars().filter(|c| matches!(c, '0' | '#' | '?')).count();
    let min_dec = frac_pat.chars().take_while(|c| *c == '0').count();
    // POI `setExcelStyleRoundingMode`: commercial (HALF_UP), not Rust's "banker's rounding"
    let f = 10f64.powi(i32::try_from(decimals).unwrap_or(0));
    let s = format!("{:.*}", decimals, (v.abs() * f).round() / f);
    let (ip, fp) = s.split_once('.').map_or((s.as_str(), ""), |(a, b)| (a, b));
    let mut ip = ip.trim_start_matches('0').to_owned();
    while ip.len() < min_int {
        ip.insert(0, '0');
    }
    if grouping {
        let digits: Vec<char> = ip.chars().collect();
        let mut g = String::new();
        for (k, d) in digits.iter().enumerate() {
            if k > 0 && (digits.len() - k).is_multiple_of(3) {
                g.push(',');
            }
            g.push(*d);
        }
        ip = g;
    }
    let mut fp = fp.to_owned();
    while fp.len() > min_dec && fp.ends_with('0') {
        fp.pop();
    }
    let num = if fp.is_empty() { ip } else { format!("{ip}.{fp}") };
    Some(format!("{prefix}{num}{suffix}"))
}

/// Number according to format code (not date). Unknown → [`general`].
pub fn format_number_code(v: f64, code: &str) -> String {
    if code.is_empty() || code.eq_ignore_ascii_case("General") {
        return general(v);
    }
    let secs = sections(code);
    let (sec, abs, neg_sign) = match (secs.len(), v) {
        (n, x) if n >= 3 && x == 0.0 => (secs[2], true, false),
        (n, x) if n >= 2 && x < 0.0 => (secs[1], true, false),
        (_, x) => (secs[0], x < 0.0, x < 0.0),
    };
    if sec.trim().eq_ignore_ascii_case("General") {
        return general(v);
    }
    let val = if abs { v.abs() } else { v };
    match format_plain(val, sec) {
        Some(s) => {
            let s = s.trim().to_owned();
            if neg_sign && !s.is_empty() { format!("-{s}") } else { s }
        }
        None => general(v),
    }
}

/// Cell value according to format number and custom formats (date or number).
pub fn format_cell(v: f64, id: u32, custom: &[(u32, String)], date1904: bool) -> String {
    let Some(code) = resolve(id, custom) else { return general(v) };
    if is_date_format(id, code) {
        return format_date(v, code, date1904).unwrap_or_else(|| general(v));
    }
    format_number_code(v, code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_dates_like_poi_en_us() {
        // 45366 = 2024-03-15
        assert_eq!(format_cell(45366.0, 14, &[], false), "3/15/24");
        assert_eq!(format_cell(45366.0, 15, &[], false), "15-Mar-24");
        assert_eq!(format_cell(45366.0, 17, &[], false), "Mar-24");
        assert_eq!(format_cell(45366.75, 22, &[], false), "3/15/24 18:00");
        assert_eq!(format_cell(0.5, 18, &[], false), "12:00 PM");
        assert_eq!(format_cell(0.25 + 30.0 / 86400.0, 21, &[], false), "6:00:30");
        assert_eq!(format_cell(1.5, 46, &[], false), "36:00:00");
        assert_eq!(format_cell(45366.0, 14, &[], true), "3/16/28");
    }

    #[test]
    fn custom_date_formats() {
        let c = vec![(164, "dd.mm.yyyy".to_owned()), (165, "[$-407]dddd, d. mmmm yyyy".to_owned()), (166, "yyyy-mm-dd hh:mm:ss".to_owned())];
        assert_eq!(format_cell(45366.0, 164, &c, false), "15.03.2024");
        assert_eq!(format_cell(45366.0, 165, &c, false), "Friday, 15. March 2024");
        assert_eq!(format_cell(45366.0 + 3723.0 / 86400.0, 166, &c, false), "2024-03-15 01:02:03");
        assert!(is_date_format(164, "dd.mm.yyyy"));
        assert!(!is_date_format(170, "0.00\" m\""));
        assert!(!is_date_format(2, "0.00"));
        // 1900 leap year bug: 59 = 28.02.1900, 61 = 01.03.1900
        assert_eq!(format_date(59.0, "yyyy-mm-dd", false).unwrap(), "1900-02-28");
        assert_eq!(format_date(61.0, "yyyy-mm-dd", false).unwrap(), "1900-03-01");
    }

    #[test]
    fn number_formats() {
        assert_eq!(format_cell(1234.5, 4, &[], false), "1,234.50");
        assert_eq!(format_cell(1234.5, 3, &[], false), "1,235");
        assert_eq!(format_cell(0.256, 9, &[], false), "26%");
        assert_eq!(format_cell(0.256, 10, &[], false), "25.60%");
        assert_eq!(format_cell(-5.0, 39, &[], false), "(5.00)");
        assert_eq!(format_cell(3.0, 2, &[], false), "3.00");
        assert_eq!(format_cell(45366.0, 0, &[], false), "45366");
        assert_eq!(format_cell(0.1, 0, &[], false), "0.1");
        let c = vec![(170, "#,##0.00 [$€-407]".to_owned()), (171, "0.0#".to_owned()), (172, "\"Nr. \"0".to_owned())];
        assert_eq!(format_cell(1234.5, 170, &c, false), "1,234.50 €");
        assert_eq!(format_cell(2.5, 171, &c, false), "2.5");
        assert_eq!(format_cell(2.567, 171, &c, false), "2.57");
        assert_eq!(format_cell(7.0, 172, &c, false), "Nr. 7");
        assert_eq!(format_cell(-7.0, 172, &c, false), "-Nr. 7");
        // not supported → General
        assert_eq!(format_cell(12345.0, 11, &[], false), "12345");
        assert_eq!(format_cell(0.5, 12, &[], false), "0.5");
    }
}
