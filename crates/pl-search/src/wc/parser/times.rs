// whoosh_compat/parser/times.py (whoosh-compat 0.3.0)
//
// Naive-datetime "ambiguous datetime" model. Python exceptions are modelled
// by `DErr`: `Time` = times.TimeError, `Value` = ValueError/OverflowError
// (Python's datetime range is year 1..=9999, enforced explicitly since
// chrono's range is wider).

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DErr {
    /// times.TimeError
    Time,
    /// ValueError / OverflowError
    Value,
}

pub type DResult<T> = Result<T, DErr>;

pub fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// calendar.monthrange(y, m)[1] (Python >= 3.12 handles any year).
pub fn days_in_month(y: i64, m: i64) -> DResult<i64> {
    Ok(match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => return Err(DErr::Value),
    })
}

/// Python `datetime(y, m, d, h, mi, s, us)` with its validation.
pub fn py_datetime(y: i64, m: i64, d: i64, h: i64, mi: i64, s: i64, us: i64) -> DResult<NaiveDateTime> {
    if !(1..=9999).contains(&y) || !(1..=12).contains(&m) {
        return Err(DErr::Value);
    }
    if d < 1 || d > days_in_month(y, m)? {
        return Err(DErr::Value);
    }
    if !(0..=23).contains(&h) || !(0..=59).contains(&mi) || !(0..=59).contains(&s) || !(0..=999_999).contains(&us) {
        return Err(DErr::Value);
    }
    let date = NaiveDate::from_ymd_opt(y as i32, m as u32, d as u32).ok_or(DErr::Value)?;
    let time = NaiveTime::from_hms_micro_opt(h as u32, mi as u32, s as u32, us as u32).ok_or(DErr::Value)?;
    Ok(NaiveDateTime::new(date, time))
}

/// Range check for the result of datetime arithmetic (OverflowError).
pub fn py_check(dt: NaiveDateTime) -> DResult<NaiveDateTime> {
    if (1..=9999).contains(&dt.year()) { Ok(dt) } else { Err(DErr::Value) }
}

/// `dt + timedelta(...)`
pub fn add_delta(dt: NaiveDateTime, d: Duration) -> DResult<NaiveDateTime> {
    py_check(dt.checked_add_signed(d).ok_or(DErr::Value)?)
}

pub fn micro(dt: &NaiveDateTime) -> i64 {
    i64::from(dt.nanosecond() / 1000)
}

/// times.py::adatetime
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Adt {
    pub year: Option<i64>,
    pub month: Option<i64>,
    pub day: Option<i64>,
    pub hour: Option<i64>,
    pub minute: Option<i64>,
    pub second: Option<i64>,
    pub microsecond: Option<i64>,
}

impl Adt {
    /// `adatetime(...)` with its TimeError validation.
    pub fn new(
        year: Option<i64>,
        month: Option<i64>,
        day: Option<i64>,
        hour: Option<i64>,
        minute: Option<i64>,
        second: Option<i64>,
        microsecond: Option<i64>,
    ) -> DResult<Adt> {
        if let Some(m) = month {
            if !(1..=12).contains(&m) {
                return Err(DErr::Time);
            }
        }
        if let Some(d) = day {
            if d < 1 {
                return Err(DErr::Time);
            }
        }
        if let (Some(y), Some(m), Some(d)) = (year, month, day) {
            if d > days_in_month(y, m).map_err(|_| DErr::Time)? {
                return Err(DErr::Time);
            }
        }
        let bad = |v: Option<i64>, hi: i64| v.is_some_and(|v| v < 0 || v > hi);
        if bad(hour, 23) || bad(minute, 59) || bad(second, 59) || bad(microsecond, 999_999) {
            return Err(DErr::Time);
        }
        Ok(Adt { year, month, day, hour, minute, second, microsecond })
    }

    pub fn ymd(y: i64, m: i64, d: i64) -> DResult<Adt> {
        Adt::new(Some(y), Some(m), Some(d), None, None, None, None)
    }

    fn units(&self) -> [Option<i64>; 7] {
        [self.year, self.month, self.day, self.hour, self.minute, self.second, self.microsecond]
    }

    /// adatetime.floor
    pub fn floor(&self) -> DResult<NaiveDateTime> {
        let y = self.year.ok_or(DErr::Value)?;
        py_datetime(
            y,
            self.month.unwrap_or(1),
            self.day.unwrap_or(1),
            self.hour.unwrap_or(0),
            self.minute.unwrap_or(0),
            self.second.unwrap_or(0),
            self.microsecond.unwrap_or(0),
        )
    }

    /// adatetime.ceil
    pub fn ceil(&self) -> DResult<NaiveDateTime> {
        let y = self.year.ok_or(DErr::Value)?;
        let m = self.month.unwrap_or(12);
        let d = match self.day {
            Some(d) => d,
            None => days_in_month(y, m)?,
        };
        py_datetime(
            y,
            m,
            d,
            self.hour.unwrap_or(23),
            self.minute.unwrap_or(59),
            self.second.unwrap_or(59),
            self.microsecond.unwrap_or(999_999),
        )
    }
}

/// A `datetime | adatetime` value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DL {
    Dt(NaiveDateTime),
    A(Adt),
}

impl DL {
    pub fn year(&self) -> Option<i64> {
        match self {
            DL::Dt(d) => Some(i64::from(d.year())),
            DL::A(a) => a.year,
        }
    }
    pub fn month(&self) -> Option<i64> {
        match self {
            DL::Dt(d) => Some(i64::from(d.month())),
            DL::A(a) => a.month,
        }
    }
    pub fn day(&self) -> Option<i64> {
        match self {
            DL::Dt(d) => Some(i64::from(d.day())),
            DL::A(a) => a.day,
        }
    }
    // Attribute assignment: only ever reached for an adatetime (see the
    // branch conditions in timespan.disambiguated).
    fn set_year(&mut self, v: i64) {
        if let DL::A(a) = self {
            a.year = Some(v);
        }
    }
    fn set_month(&mut self, v: Option<i64>) {
        if let DL::A(a) = self {
            a.month = v;
        }
    }
    fn set_day(&mut self, v: Option<i64>) {
        if let DL::A(a) = self {
            a.day = v;
        }
    }
}

/// times.py::floor
pub fn floor(at: &DL) -> DResult<NaiveDateTime> {
    match at {
        DL::Dt(d) => Ok(*d),
        DL::A(a) => a.floor(),
    }
}

/// times.py::ceil
pub fn ceil(at: &DL) -> DResult<NaiveDateTime> {
    match at {
        DL::Dt(d) => Ok(*d),
        DL::A(a) => a.ceil(),
    }
}

/// times.py::has_no_date
pub fn has_no_date(at: &DL) -> bool {
    match at {
        DL::Dt(_) => false,
        DL::A(a) => a.year.is_none() && a.month.is_none() && a.day.is_none(),
    }
}

/// times.py::has_no_time
pub fn has_no_time_adt(a: &Adt) -> bool {
    a.hour.is_none() && a.minute.is_none() && a.second.is_none() && a.microsecond.is_none()
}

/// times.py::is_ambiguous
pub fn is_ambiguous(at: &DL) -> bool {
    match at {
        DL::Dt(_) => false,
        DL::A(a) => a.units().iter().any(Option::is_none),
    }
}

/// times.py::fix
pub fn fix(a: Adt) -> DResult<DL> {
    if is_ambiguous(&DL::A(a)) {
        return Ok(DL::A(a));
    }
    let u = a.units();
    Ok(DL::Dt(py_datetime(
        u[0].unwrap_or(0),
        u[1].unwrap_or(0),
        u[2].unwrap_or(0),
        u[3].unwrap_or(0),
        u[4].unwrap_or(0),
        u[5].unwrap_or(0),
        u[6].unwrap_or(0),
    )?))
}

/// _UnitSpan subclasses (CalendarPeriod / TimeOnPeriod).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanKind {
    Plain,
    CalendarPeriod(&'static str),
    TimeOnPeriod(&'static str),
}

/// times.py::timespan
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub start: DL,
    pub end: DL,
    pub bounds_swapped: bool,
    pub kind: SpanKind,
}

impl Span {
    pub fn new(start: DL, end: DL) -> Span {
        Span { start, end, bounds_swapped: false, kind: SpanKind::Plain }
    }

    /// TimeOnPeriod.of
    pub fn time_on_period_of(&self) -> Span {
        let unit = match self.kind {
            SpanKind::CalendarPeriod(u) | SpanKind::TimeOnPeriod(u) => u,
            SpanKind::Plain => "period",
        };
        Span { start: self.start, end: self.end, bounds_swapped: false, kind: SpanKind::TimeOnPeriod(unit) }
    }

    /// timespan.disambiguated
    pub fn disambiguated(&self, basedate: NaiveDateTime) -> DResult<Span> {
        let mut start = self.start;
        let mut end = self.end;
        let start_year_was_amb = start.year().is_none();
        let end_year_was_amb = end.year().is_none();
        let mut swapped = false;
        let by = i64::from(basedate.year());
        let bm = i64::from(basedate.month());
        let bd = i64::from(basedate.day());

        if has_no_date(&start) && has_no_date(&end) {
            for x in [&mut start, &mut end] {
                x.set_year(by);
                x.set_month(Some(bm));
                x.set_day(Some(bd));
            }
        } else if start.year().is_none() && end.year().is_none() {
            start.set_year(by);
            end.set_year(by);
        } else if start.year().is_none() {
            if let Some(y) = end.year() {
                start.set_year(y);
            }
        } else if end.year().is_none() {
            let sy = start.year().unwrap_or(by);
            end.set_year(sy.max(by));
        }

        if start.year() == end.year() {
            let start_dm = !(start.month().is_none() && start.day().is_none());
            let end_dm = !(end.month().is_none() && end.day().is_none());
            if end_dm && !start_dm {
                if floor(&start)?.time() > ceil(&end)?.time() {
                    start.set_month(Some(bm));
                    start.set_day(Some(bd));
                } else {
                    start.set_month(end.month());
                    start.set_day(end.day());
                }
            } else if start_dm && !end_dm {
                end.set_month(Some(bm));
                end.set_day(Some(bd));
            }
        }

        if floor(&start)?.date() > ceil(&end)?.date() {
            if start_year_was_amb {
                let ey = end.year().ok_or(DErr::Value)?;
                start.set_year(ey - 1);
            } else if end_year_was_amb {
                let sy = start.year().ok_or(DErr::Value)?;
                end.set_year(sy + 1);
            } else {
                std::mem::swap(&mut start, &mut end);
                swapped = true;
            }
        }

        let mut s = floor(&start)?;
        let mut e = ceil(&end)?;

        if s.date() == e.date() && s.time() > e.time() {
            if matches!(self.start, DL::Dt(_)) && matches!(self.end, DL::Dt(_)) {
                std::mem::swap(&mut s, &mut e);
                swapped = true;
            } else {
                e = add_delta(e, Duration::days(1))?;
            }
        }
        Ok(Span { start: DL::Dt(s), end: DL::Dt(e), bounds_swapped: swapped, kind: SpanKind::Plain })
    }
}

/// A grammar result: `datetime | adatetime | timespan`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DV {
    Dt(NaiveDateTime),
    A(Adt),
    S(Span),
}

impl DV {
    pub fn from_dl(d: DL) -> DV {
        match d {
            DL::Dt(x) => DV::Dt(x),
            DL::A(a) => DV::A(a),
        }
    }
}

/// times.py::is_void
pub fn is_void(at: &DV) -> bool {
    match at {
        DV::A(a) => a.units().iter().all(Option::is_none),
        _ => false,
    }
}

/// times.py::fill_in(at, basedate)
pub fn fill_in(at: &DV, base: &DV) -> DResult<DV> {
    let a = match at {
        DV::Dt(_) => return Ok(*at),
        DV::S(span) => return Ok(DV::S(span.time_on_period_of())),
        DV::A(a) => a,
    };
    let b = match base {
        DV::S(bs) => {
            if a.units().iter().any(Option::is_some) {
                return Ok(DV::S(bs.time_on_period_of()));
            }
            return Ok(*base);
        }
        DV::A(b) => *b,
        DV::Dt(d) => Adt {
            year: Some(i64::from(d.year())),
            month: Some(i64::from(d.month())),
            day: Some(i64::from(d.day())),
            hour: Some(i64::from(d.hour())),
            minute: Some(i64::from(d.minute())),
            second: Some(i64::from(d.second())),
            microsecond: Some(micro(d)),
        },
    };
    let n = Adt::new(
        a.year.or(b.year),
        a.month.or(b.month),
        a.day.or(b.day),
        a.hour.or(b.hour),
        a.minute.or(b.minute),
        a.second.or(b.second),
        a.microsecond.or(b.microsecond),
    )?;
    Ok(DV::from_dl(fix(n)?))
}

/// adatetime.disambiguated / timespan.disambiguated on a DV.
pub fn disambiguated(v: &DV, basedate: NaiveDateTime) -> DResult<DV> {
    match v {
        DV::Dt(_) => Ok(*v),
        DV::A(a) => {
            if !is_ambiguous(&DL::A(*a)) {
                return Ok(DV::from_dl(fix(*a)?));
            }
            Ok(DV::S(Span::new(DL::A(*a), DL::A(*a)).disambiguated(basedate)?))
        }
        DV::S(s) => Ok(DV::S(s.disambiguated(basedate)?)),
    }
}

/// times.py::relative_days
pub fn relative_days(current_wday: i64, wday: i64, dir: i64) -> i64 {
    if current_wday == wday {
        return 7 * dir;
    }
    if dir == 1 { (wday + 7 - current_wday) % 7 } else { -((current_wday + 7 - wday) % 7) }
}
