//! `date.strftime` / `datetime.strftime` of CPython 3.12 on glibc (locale "C"/POSIX, as in the
//! upstream container). Unknown directives stay unchanged like in glibc.
//!
//! upstream: CPython Modules/timemodule.c + Lib/_strptime/datetime (`%f`, `%z`, `%Z`, `%:z`),
//! glibc time/strftime_l.c.

use chrono::{Datelike, FixedOffset, NaiveDateTime, Timelike};

const DAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October",
    "November", "December",
];

/// Python time zone information of a value.
#[derive(Debug, Clone, Copy)]
pub enum TzInfo {
    /// `date` or naive `datetime`: `%z`/`%Z` empty.
    Naive,
    /// `datetime` with fixed offset; `utc_name` = tzname is "UTC" (datetime.timezone.utc).
    Fixed(FixedOffset),
}

fn tz_name(off: FixedOffset) -> String {
    // datetime.timezone(...).tzname(): "UTC" or "UTC+02:00"
    let secs = off.local_minus_utc();
    if secs == 0 {
        return "UTC".into();
    }
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.abs();
    let (h, m, s) = (a / 3600, (a % 3600) / 60, a % 60);
    if s != 0 {
        format!("UTC{sign}{h:02}:{m:02}:{s:02}")
    } else {
        format!("UTC{sign}{h:02}:{m:02}")
    }
}

fn utcoffset(off: FixedOffset, colon: bool) -> String {
    let secs = off.local_minus_utc();
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.abs();
    let (h, m, s) = (a / 3600, (a % 3600) / 60, a % 60);
    let sep = if colon { ":" } else { "" };
    if s != 0 {
        format!("{sign}{h:02}{sep}{m:02}{sep}{s:02}")
    } else {
        format!("{sign}{h:02}{sep}{m:02}")
    }
}

fn iso_week(dt: &NaiveDateTime) -> (i32, u32) {
    let w = dt.date().iso_week();
    (w.year(), w.week())
}

/// `%U` / `%W`.
fn week_number(dt: &NaiveDateTime, monday_first: bool) -> u32 {
    let yday = dt.ordinal0() as i64; // 0-based
    let wday = if monday_first {
        dt.weekday().num_days_from_monday() as i64
    } else {
        dt.weekday().num_days_from_sunday() as i64
    };
    ((yday + 7 - wday) / 7) as u32
}

/// Python-`strftime`.
pub fn strftime(dt: &NaiveDateTime, tz: TzInfo, fmt: &str) -> String {
    let mut out = String::with_capacity(fmt.len() + 16);
    let chars: Vec<char> = fmt.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c != '%' {
            out.push(c);
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        // glibc flags and width
        let mut flag: Option<char> = None;
        let mut upper = false;
        while i < chars.len() && matches!(chars[i], '-' | '_' | '0' | '^' | '#') {
            if chars[i] == '^' {
                upper = true;
            } else {
                flag = Some(chars[i]);
            }
            i += 1;
        }
        let mut width: Option<usize> = None;
        while i < chars.len() && chars[i].is_ascii_digit() {
            width = Some(width.unwrap_or(0).saturating_mul(10).saturating_add(chars[i].to_digit(10).unwrap_or(0) as usize));
            i += 1;
        }
        // CPython time.strftime returns "" if the result does not fit in 256 × len(format) characters
        // (glibc reports 0). Without this limit "%999999999d" reserved ~1 GB (fuzzing finding
        // S-8, results/release/security/BERICHT.md); "%99999999999999999999d" overflowed.
        if width.is_some_and(|w| w > 256 * chars.len()) {
            return String::new();
        }
        // Python: "%:z"
        if i + 1 < chars.len() && chars[i] == ':' && chars[i + 1] == 'z' && flag.is_none() && width.is_none() {
            if let TzInfo::Fixed(off) = tz {
                out.push_str(&utcoffset(off, true));
            }
            i += 2;
            continue;
        }
        if i >= chars.len() {
            out.extend(&chars[start..]);
            break;
        }
        let d = chars[i];
        i += 1;
        let num = |v: i64, w: usize, pad: char| -> String {
            let (pad, w) = match flag {
                Some('-') => (' ', 0),
                Some('_') => (' ', width.unwrap_or(w)),
                Some('0') => ('0', width.unwrap_or(w)),
                _ => (pad, width.unwrap_or(w)),
            };
            let s = v.abs().to_string();
            let sign = if v < 0 { "-" } else { "" };
            let len = s.len() + sign.len();
            if len >= w {
                format!("{sign}{s}")
            } else if pad == '0' {
                format!("{sign}{}{s}", "0".repeat(w - len))
            } else {
                format!("{}{sign}{s}", " ".repeat(w - len))
            }
        };
        let text = |s: String| -> String {
            let s = if upper {
                s.to_uppercase()
            } else if flag == Some('#') {
                // glibc: "opposite case" – %p/%Z lowercase, otherwise uppercase
                if d == 'p' || d == 'Z' { s.to_lowercase() } else { s.to_uppercase() }
            } else {
                s
            };
            match width {
                Some(w) if s.chars().count() < w => {
                    let padc = if flag == Some('0') { "0" } else { " " };
                    format!("{}{s}", padc.repeat(w - s.chars().count()))
                }
                _ => s,
            }
        };
        let hour12 = {
            let h = dt.hour() % 12;
            if h == 0 { 12 } else { h }
        };
        let piece = match d {
            '%' => "%".to_string(),
            'a' => text(DAYS[dt.weekday().num_days_from_monday() as usize][..3].to_string()),
            'A' => text(DAYS[dt.weekday().num_days_from_monday() as usize].to_string()),
            'b' | 'h' => text(MONTHS[dt.month0() as usize][..3].to_string()),
            'B' => text(MONTHS[dt.month0() as usize].to_string()),
            'c' => text(strftime(dt, tz, "%a %b %e %H:%M:%S %Y")),
            'C' => num((dt.year() as i64).div_euclid(100), 2, '0'),
            'd' => num(dt.day() as i64, 2, '0'),
            'D' => text(strftime(dt, tz, "%m/%d/%y")),
            'e' => num(dt.day() as i64, 2, ' '),
            'f' => format!("{:06}", dt.nanosecond() / 1000),
            'F' => text(strftime(dt, tz, "%Y-%m-%d")),
            'g' => num((iso_week(dt).0 as i64).rem_euclid(100), 2, '0'),
            'G' => num(iso_week(dt).0 as i64, 1, '0'),
            'H' => num(dt.hour() as i64, 2, '0'),
            'I' => num(hour12 as i64, 2, '0'),
            'j' => num(dt.ordinal() as i64, 3, '0'),
            'k' => num(dt.hour() as i64, 2, ' '),
            'l' => num(hour12 as i64, 2, ' '),
            'm' => num(dt.month() as i64, 2, '0'),
            'M' => num(dt.minute() as i64, 2, '0'),
            'n' => "\n".into(),
            'p' => text(if dt.hour() < 12 { "AM" } else { "PM" }.into()),
            'P' => text(if dt.hour() < 12 { "am" } else { "pm" }.into()),
            'r' => text(strftime(dt, tz, "%I:%M:%S %p")),
            'R' => text(strftime(dt, tz, "%H:%M")),
            's' => {
                let base = match tz {
                    TzInfo::Fixed(off) => dt.and_utc().timestamp() - off.local_minus_utc() as i64,
                    // naive: local time of the process (container: UTC)
                    TzInfo::Naive => dt.and_utc().timestamp(),
                };
                num(base, 1, '0')
            }
            'S' => num(dt.second() as i64, 2, '0'),
            't' => "\t".into(),
            'T' => text(strftime(dt, tz, "%H:%M:%S")),
            'u' => num(dt.weekday().number_from_monday() as i64, 1, '0'),
            'U' => num(week_number(dt, false) as i64, 2, '0'),
            'V' => num(iso_week(dt).1 as i64, 2, '0'),
            'w' => num(dt.weekday().num_days_from_sunday() as i64, 1, '0'),
            'W' => num(week_number(dt, true) as i64, 2, '0'),
            'x' => text(strftime(dt, tz, "%m/%d/%y")),
            'X' => text(strftime(dt, tz, "%H:%M:%S")),
            'y' => num((dt.year() as i64).rem_euclid(100), 2, '0'),
            'Y' => num(dt.year() as i64, 1, '0'),
            'z' => match tz {
                TzInfo::Fixed(off) => utcoffset(off, false),
                TzInfo::Naive => String::new(),
            },
            'Z' => match tz {
                TzInfo::Fixed(off) => tz_name(off),
                TzInfo::Naive => String::new(),
            },
            _ => chars[start..i].iter().collect(),
        };
        out.push_str(&piece);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(h, mi, s).unwrap()
    }

    #[test]
    fn basic() {
        let x = dt(1989, 12, 2, 0, 0, 0);
        assert_eq!(strftime(&x, TzInfo::Naive, "%y/%b/%B/%d"), "89/Dec/December/02");
        assert_eq!(strftime(&x, TzInfo::Naive, "%H:%M"), "00:00");
        assert_eq!(strftime(&dt(2023, 10, 26, 14, 30, 5), TzInfo::Naive, "%A %a %j %-d %Q %%"), "Thursday Thu 299 26 %Q %");
        let off = FixedOffset::east_opt(7200).unwrap();
        assert_eq!(strftime(&x, TzInfo::Fixed(off), "%z %Z %:z"), "+0200 UTC+02:00 +02:00");
        assert_eq!(strftime(&dt(1232, 1, 9, 1, 1, 1), TzInfo::Naive, "%Y-%m-%d"), "1232-01-09");
        assert_eq!(strftime(&dt(2024, 1, 1, 13, 0, 0), TzInfo::Naive, "%U %W %V %G %I%p"), "00 01 01 2024 01PM");
    }

    /// Fuzzing finding S-8: huge widths must neither overflow nor allocate huge buffers.
    #[test]
    fn huge_width_is_bounded() {
        let x = dt(2024, 1, 1, 13, 0, 0);
        assert_eq!(strftime(&x, TzInfo::Naive, "%999999999d"), "");
        assert_eq!(strftime(&x, TzInfo::Naive, "%99999999999999999999999t"), "");
        assert_eq!(strftime(&x, TzInfo::Naive, "%_999999999A x"), "");
        assert_eq!(strftime(&x, TzInfo::Naive, "%10d"), "0000000001");
        assert_eq!(strftime(&x, TzInfo::Naive, "%05Y"), "02024");
    }
}
