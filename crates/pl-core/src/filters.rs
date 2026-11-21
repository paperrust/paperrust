//! Generic filters like django-filter and ordering like DRF `OrderingFilter`.
//!
//! upstream: django_filters/filterset.py@26.2 (`FilterSet.get_filters`, `filter_for_lookup`),
//! django_filters/rest_framework/{filterset,filters,backends}.py (`BooleanFilter` with
//! `BooleanWidget`, `IsoDateTimeFilter` for `DateTimeField`, `translate_validation`),
//! django_filters/fields.py (`BaseCSVField`), django_filters/widgets.py (`BooleanWidget`,
//! `BaseCSVWidget`), django/forms/fields.py@5.2 (`CharField`, `DecimalField`, `DateField`),
//! django/db/backends/postgresql/{operations,base}.py (`lookup_cast`, `operators`),
//! django/db/models/lookups.py (`IntegerFieldOverflow`, `In`),
//! rest_framework/filters.py@3.18.3 (`OrderingFilter`),
//! src/documents/filters.py@v3.4.2 (`CHAR_KWARGS`, `ID_KWARGS`, `INT_KWARGS`, `DATE_KWARGS`,
//! `DATETIME_KWARGS`).
//!
//! ## Behavior (short)
//! * Parameter name: `field` for `exact`, otherwise `field__lookup` (e.g. `name__icontains`, `id__in`).
//! * Value = **last** query value. Empty value → filter is dropped.
//! * Errors (all filters are checked) → 400 `{"param": ["message"]}` in filter order.
//! * Numbers: Python `Decimal` (`"1.5"`, `"1e3"`, `" 2 "` ok; `NaN`/`Inf`/text → `Enter a number.`),
//!   truncated to an integer for integer columns (`int(Decimal)`); values outside the
//!   column range like Django's `IntegerFieldOverflow` (empty or full result).
//! * `__in`: comma-separated, empty entries ignored; only empty entries → no hits.
//! * `__isnull` and bool fields: `BooleanWidget` (`true/false/1/0`, case-insensitive); anything else → filter is dropped.
//! * Text: `CharField(strip=True)`; NUL → `Null characters are not allowed.`.
//!   `iexact`→`UPPER(col::text) = UPPER(v)`, `icontains`→`UPPER(col::text) LIKE UPPER('%v%')`
//!   (with `\` escape of `\ % _`), `istartswith`/`iendswith` accordingly (like Django on PG).
//! * Date (`DateField`): locale input formats of the active language; DateTime: ISO
//!   (`IsoDateTimeFilter`), naive values in `PAPERLESS_TIME_ZONE`.

use chrono_tz::Tz;
use sea_query::{Alias, BinOper, Expr, Func, Order, SelectStatement, SimpleExpr};

use crate::datetime::{parse_date_input, parse_iso_datetime_input};
use crate::{ApiError, QueryParams, RequestContext, ValidationErrors};

/// Lookup of a filter (`lookup_expr`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lookup {
    Exact,
    IExact,
    Contains,
    IContains,
    StartsWith,
    IStartsWith,
    EndsWith,
    IEndsWith,
    In,
    IsNull,
    Gt,
    Gte,
    Lt,
    Lte,
    /// `__year` (number, on `DateTimeField` in `TIME_ZONE`).
    Year,
    /// `__month`.
    Month,
    /// `__day`.
    Day,
    /// `__date__gt` (date in `TIME_ZONE`, `DateTimeField` only).
    DateGt,
    DateGte,
    DateLt,
    DateLte,
}

impl Lookup {
    /// Django lookup name (parameter suffix).
    pub fn as_str(self) -> &'static str {
        match self {
            Lookup::Exact => "exact",
            Lookup::IExact => "iexact",
            Lookup::Contains => "contains",
            Lookup::IContains => "icontains",
            Lookup::StartsWith => "startswith",
            Lookup::IStartsWith => "istartswith",
            Lookup::EndsWith => "endswith",
            Lookup::IEndsWith => "iendswith",
            Lookup::In => "in",
            Lookup::IsNull => "isnull",
            Lookup::Gt => "gt",
            Lookup::Gte => "gte",
            Lookup::Lt => "lt",
            Lookup::Lte => "lte",
            Lookup::Year => "year",
            Lookup::Month => "month",
            Lookup::Day => "day",
            Lookup::DateGt => "date__gt",
            Lookup::DateGte => "date__gte",
            Lookup::DateLt => "date__lt",
            Lookup::DateLte => "date__lte",
        }
    }

    /// django-filter `get_filter_name(field_name, lookup_expr)`: `exact` → field name only.
    pub fn param_name(self, field: &str) -> String {
        if self == Lookup::Exact { field.to_string() } else { format!("{field}__{}", self.as_str()) }
    }
}

/// upstream `CHAR_KWARGS`.
pub const CHAR_KWARGS: &[Lookup] = &[Lookup::IStartsWith, Lookup::IEndsWith, Lookup::IContains, Lookup::IExact];
/// upstream `ID_KWARGS`.
pub const ID_KWARGS: &[Lookup] = &[Lookup::In, Lookup::Exact];
/// upstream `INT_KWARGS`.
pub const INT_KWARGS: &[Lookup] = &[Lookup::Exact, Lookup::Gt, Lookup::Gte, Lookup::Lt, Lookup::Lte, Lookup::IsNull];
/// upstream `DATE_KWARGS`.
pub const DATE_KWARGS: &[Lookup] = &[Lookup::Year, Lookup::Month, Lookup::Day, Lookup::Gt, Lookup::Gte, Lookup::Lt, Lookup::Lte];
/// upstream `DATETIME_KWARGS`.
pub const DATETIME_KWARGS: &[Lookup] = &[
    Lookup::Year,
    Lookup::Month,
    Lookup::Day,
    Lookup::DateGt,
    Lookup::DateGte,
    Lookup::Gt,
    Lookup::Gte,
    Lookup::DateLt,
    Lookup::DateLte,
    Lookup::Lt,
    Lookup::Lte,
];

/// Type of the model field (determines parser and SQL).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// Integer with the value range of the DB column (`connection.ops.integer_field_range`).
    Integer { min: i64, max: i64 },
    /// `CharField`/`TextField`.
    Char,
    /// `BooleanField`.
    Boolean,
    /// `DateField`.
    Date,
    /// `DateTimeField` (timestamptz).
    DateTime,
}

impl FieldKind {
    /// `AutoField`/`IntegerField`/`ForeignKey` (int4).
    pub const INT: FieldKind = FieldKind::Integer { min: i32::MIN as i64, max: i32::MAX as i64 };
    /// `BigAutoField`/`BigIntegerField` (int8).
    pub const BIGINT: FieldKind = FieldKind::Integer { min: i64::MIN, max: i64::MAX };
    /// `PositiveIntegerField` (Django PG range 0..2147483647).
    pub const POSITIVE_INT: FieldKind = FieldKind::Integer { min: 0, max: i32::MAX as i64 };
    /// `PositiveSmallIntegerField` (0..32767).
    pub const POSITIVE_SMALLINT: FieldKind = FieldKind::Integer { min: 0, max: i16::MAX as i64 };
    /// `SmallIntegerField`.
    pub const SMALLINT: FieldKind = FieldKind::Integer { min: i16::MIN as i64, max: i16::MAX as i64 };
}

/// Context for value parsing.
#[derive(Debug, Clone, Copy)]
pub struct FilterContext<'a> {
    /// `PAPERLESS_TIME_ZONE`.
    pub tz: &'a Tz,
    /// Active language (date formats).
    pub language: &'a str,
}

impl<'a> FilterContext<'a> {
    /// From request and time zone.
    pub fn new(ctx: &'a RequestContext, tz: &'a Tz) -> Self {
        Self { tz, language: ctx.language }
    }
}

/// Integer after `int(Decimal(s))`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntValue {
    Value(i64),
    /// Magnitude too large for i64 (sign: `true` = negative).
    Overflow {
        negative: bool,
    },
}

/// Message `DecimalField.invalid`.
pub const MSG_ENTER_NUMBER: &str = "Enter a number.";
/// Message `ProhibitNullCharactersValidator`.
pub const MSG_NULL_CHARS: &str = "Null characters are not allowed.";

/// Django `forms.DecimalField.clean` + `int()`: `Ok(None)` for an empty value.
pub fn parse_number(value: &str) -> Result<Option<IntValue>, &'static str> {
    if value.is_empty() {
        return Ok(None);
    }
    let s = value.trim();
    let err = Err(MSG_ENTER_NUMBER);
    let (neg, body) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        Some(_) => (false, s),
        None => return err,
    };
    let (mant, exp) = match body.find(['e', 'E']) {
        Some(i) => (&body[..i], Some(&body[i + 1..])),
        None => (body, None),
    };
    let valid_digits = |d: &str| -> bool {
        !d.starts_with('_') && !d.ends_with('_') && !d.contains("__") && d.bytes().all(|b| b.is_ascii_digit() || b == b'_')
    };
    let (ip, fp) = match mant.split_once('.') {
        Some((i, f)) => (i, f),
        None => (mant, ""),
    };
    if (ip.is_empty() && fp.is_empty()) || !valid_digits(ip) || !valid_digits(fp) {
        return err; // also NaN/Infinity → "Enter a number."
    }
    let exp: i64 = match exp {
        Some(e) => {
            let (en, eb) = match e.as_bytes().first() {
                Some(b'-') => (true, &e[1..]),
                Some(b'+') => (false, &e[1..]),
                _ => (false, e),
            };
            if eb.is_empty() || !valid_digits(eb) {
                return err;
            }
            let v: i64 = eb.replace('_', "").parse().unwrap_or(i64::MAX / 4);
            if en { -v } else { v }
        }
        None => 0,
    };
    let digits: String = format!("{}{}", ip.replace('_', ""), fp.replace('_', ""));
    let point = ip.replace('_', "").len() as i64 + exp; // digits before the decimal point
    let int_digits: String = if point <= 0 {
        String::new()
    } else if point as usize >= digits.len() {
        if digits.trim_start_matches('0').is_empty() {
            String::new()
        } else if point > 40 {
            return Ok(Some(IntValue::Overflow { negative: neg }));
        } else {
            format!("{digits}{}", "0".repeat(point as usize - digits.len()))
        }
    } else {
        digits[..point as usize].to_string()
    };
    let t = int_digits.trim_start_matches('0');
    if t.is_empty() {
        return Ok(Some(IntValue::Value(0)));
    }
    match t.parse::<i128>() {
        Ok(v) => {
            let v = if neg { -v } else { v };
            Ok(Some(i64::try_from(v).map(IntValue::Value).unwrap_or(IntValue::Overflow { negative: neg })))
        }
        Err(_) => Ok(Some(IntValue::Overflow { negative: neg })),
    }
}

/// django-filter `BooleanWidget.value_from_datadict`.
pub fn parse_bool_widget(value: Option<&str>) -> Option<bool> {
    match value.map(str::to_lowercase).as_deref() {
        Some("1") | Some("true") => Some(true),
        Some("0") | Some("false") => Some(false),
        _ => None,
    }
}

/// Django `forms.CharField(strip=True)` for filters: `Ok(None)` if empty.
pub fn parse_char(value: &str) -> Result<Option<String>, &'static str> {
    let v = value.trim();
    if v.contains('\0') {
        return Err(MSG_NULL_CHARS);
    }
    if v.is_empty() { Ok(None) } else { Ok(Some(v.to_string())) }
}

/// Django `prep_for_like_query`: escape `\`, `%`, `_` with `\`.
pub fn escape_like(v: &str) -> String {
    v.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

fn false_expr() -> SimpleExpr {
    Expr::cust("1 = 0")
}

fn true_expr() -> SimpleExpr {
    Expr::cust("1 = 1")
}

fn upper_text(e: &SimpleExpr) -> SimpleExpr {
    SimpleExpr::FunctionCall(Func::upper(Expr::expr(e.clone()).cast_as(Alias::new("text"))))
}

/// SQL condition for a text lookup like Django on PostgreSQL.
pub fn char_condition(col: &SimpleExpr, lookup: Lookup, v: &str) -> Option<SimpleExpr> {
    let text = || Expr::expr(col.clone()).cast_as(Alias::new("text"));
    let up = |p: String| SimpleExpr::FunctionCall(Func::upper(Expr::val(p)));
    Some(match lookup {
        Lookup::Exact => Expr::expr(col.clone()).eq(v),
        Lookup::IExact => Expr::expr(upper_text(col)).eq(up(v.to_string())),
        Lookup::Contains => Expr::expr(text()).binary(BinOper::Like, Expr::val(format!("%{}%", escape_like(v)))),
        Lookup::IContains => Expr::expr(upper_text(col)).binary(BinOper::Like, up(format!("%{}%", escape_like(v)))),
        Lookup::StartsWith => Expr::expr(text()).binary(BinOper::Like, Expr::val(format!("{}%", escape_like(v)))),
        Lookup::IStartsWith => Expr::expr(upper_text(col)).binary(BinOper::Like, up(format!("{}%", escape_like(v)))),
        Lookup::EndsWith => Expr::expr(text()).binary(BinOper::Like, Expr::val(format!("%{}", escape_like(v)))),
        Lookup::IEndsWith => Expr::expr(upper_text(col)).binary(BinOper::Like, up(format!("%{}", escape_like(v)))),
        Lookup::Gt => Expr::expr(col.clone()).gt(v),
        Lookup::Gte => Expr::expr(col.clone()).gte(v),
        Lookup::Lt => Expr::expr(col.clone()).lt(v),
        Lookup::Lte => Expr::expr(col.clone()).lte(v),
        _ => return None,
    })
}

/// SQL condition for integer comparisons incl. `IntegerFieldOverflow`.
pub fn int_condition(col: &SimpleExpr, lookup: Lookup, v: IntValue, min: i64, max: i64) -> SimpleExpr {
    let (val, under, over) = match v {
        IntValue::Value(x) => (x, x < min, x > max),
        IntValue::Overflow { negative } => (0, negative, !negative),
    };
    let c = Expr::expr(col.clone());
    match lookup {
        Lookup::Exact => {
            if under || over {
                false_expr()
            } else {
                c.eq(val)
            }
        }
        Lookup::Gt => {
            if over {
                false_expr()
            } else if under {
                true_expr()
            } else {
                c.gt(val)
            }
        }
        Lookup::Gte => {
            if over {
                false_expr()
            } else if under {
                true_expr()
            } else {
                c.gte(val)
            }
        }
        Lookup::Lt => {
            if under {
                false_expr()
            } else if over {
                true_expr()
            } else {
                c.lt(val)
            }
        }
        Lookup::Lte => {
            if under {
                false_expr()
            } else if over {
                true_expr()
            } else {
                c.lte(val)
            }
        }
        _ => false_expr(),
    }
}

fn extract(part: &str, col: &SimpleExpr, kind: FieldKind, tz: &Tz) -> SimpleExpr {
    match kind {
        FieldKind::DateTime => {
            Expr::cust_with_exprs(format!("EXTRACT({part} FROM $1 AT TIME ZONE $2)"), [col.clone(), Expr::val(tz.name()).into()])
        }
        _ => Expr::cust_with_exprs(format!("EXTRACT({part} FROM $1)"), [col.clone()]),
    }
}

fn local_date(col: &SimpleExpr, tz: &Tz) -> SimpleExpr {
    Expr::cust_with_exprs("($1 AT TIME ZONE $2)::date", [col.clone(), Expr::val(tz.name()).into()])
}

/// A single filter (parameter name, column, type, lookup).
#[derive(Debug, Clone)]
pub struct FilterDef {
    pub param: String,
    pub expr: SimpleExpr,
    pub kind: FieldKind,
    pub lookup: Lookup,
}

/// Set of filters like a django-filter `FilterSet` (fields/lookups only; custom
/// filter logic is implemented additionally by the domain crate).
///
/// ```
/// use pl_core::filters::{FilterSet, FieldKind, ID_KWARGS, CHAR_KWARGS, FilterContext};
/// use pl_core::sea_query::{Expr, Alias};
/// let fs = FilterSet::new()
///     .field("id", Expr::col(Alias::new("id")), FieldKind::INT, ID_KWARGS)
///     .field("name", Expr::col(Alias::new("name")), FieldKind::Char, CHAR_KWARGS);
/// assert_eq!(fs.params(), vec!["id__in", "id", "name__istartswith", "name__iendswith", "name__icontains", "name__iexact"]);
/// ```
#[derive(Debug, Clone, Default)]
pub struct FilterSet {
    defs: Vec<FilterDef>,
}

impl FilterSet {
    /// Empty.
    pub fn new() -> Self {
        Self::default()
    }

    /// `Meta.fields = {name: lookups}` – one filter per lookup, order as given.
    pub fn field(mut self, name: &str, expr: impl Into<SimpleExpr>, kind: FieldKind, lookups: &[Lookup]) -> Self {
        let expr = expr.into();
        for l in lookups {
            self.upsert(FilterDef { param: l.param_name(name), expr: expr.clone(), kind, lookup: *l });
        }
        self
    }

    /// Declared filter (`is_root = BooleanFilter(field_name="tn_parent", lookup_expr="isnull")`).
    /// The same name replaces the Meta filter at its position (OrderedDict `update`).
    pub fn declared(mut self, param: &str, expr: impl Into<SimpleExpr>, kind: FieldKind, lookup: Lookup) -> Self {
        self.upsert(FilterDef { param: param.to_string(), expr: expr.into(), kind, lookup });
        self
    }

    fn upsert(&mut self, d: FilterDef) {
        if let Some(e) = self.defs.iter_mut().find(|e| e.param == d.param) {
            *e = d;
        } else {
            self.defs.push(d);
        }
    }

    /// Parameter names in filter order.
    pub fn params(&self) -> Vec<&str> {
        self.defs.iter().map(|d| d.param.as_str()).collect()
    }

    /// All filters.
    pub fn defs(&self) -> &[FilterDef] {
        &self.defs
    }

    /// Validates all parameters and returns the WHERE conditions (to be applied AND-combined).
    /// Errors → 400 in django-filter format.
    pub fn conditions(&self, q: &QueryParams, ctx: &FilterContext<'_>) -> Result<Vec<SimpleExpr>, ApiError> {
        let mut errors = ValidationErrors::new();
        let mut out = Vec::new();
        for d in &self.defs {
            match build_condition(d, q.get(&d.param), ctx) {
                Ok(Some(c)) => out.push(c),
                Ok(None) => {}
                Err(msg) => errors.add(&d.param, msg),
            }
        }
        errors.into_result()?;
        Ok(out)
    }

    /// Like [`FilterSet::conditions`], applied directly to a `SELECT`.
    pub fn apply(&self, select: &mut SelectStatement, q: &QueryParams, ctx: &FilterContext<'_>) -> Result<(), ApiError> {
        for c in self.conditions(q, ctx)? {
            select.and_where(c);
        }
        Ok(())
    }
}

/// Builds the condition of a filter from the (last) query value. `Ok(None)` = filter is dropped.
pub fn build_condition(d: &FilterDef, raw: Option<&str>, ctx: &FilterContext<'_>) -> Result<Option<SimpleExpr>, String> {
    let col = &d.expr;
    if d.lookup == Lookup::IsNull {
        return Ok(parse_bool_widget(raw).map(
            |b| {
                if b { Expr::expr(col.clone()).is_null() } else { Expr::expr(col.clone()).is_not_null() }
            },
        ));
    }
    if d.kind == FieldKind::Boolean && d.lookup == Lookup::Exact {
        return Ok(parse_bool_widget(raw).map(|b| Expr::expr(col.clone()).eq(b)));
    }
    let Some(raw) = raw else { return Ok(None) };
    match d.lookup {
        Lookup::In => {
            if raw.is_empty() {
                return Ok(None);
            }
            match d.kind {
                FieldKind::Integer { min, max } => {
                    let mut vals = Vec::new();
                    let mut any = false;
                    for item in raw.split(',') {
                        if let Some(v) = parse_number(item).map_err(String::from)? {
                            any = true;
                            if let IntValue::Value(x) = v
                                && (min..=max).contains(&x)
                            {
                                vals.push(x);
                            }
                        }
                    }
                    let _ = any;
                    Ok(Some(if vals.is_empty() { false_expr() } else { Expr::expr(col.clone()).is_in(vals) }))
                }
                FieldKind::Char => {
                    let mut vals = Vec::new();
                    for item in raw.split(',') {
                        if let Some(v) = parse_char(item).map_err(String::from)? {
                            vals.push(v);
                        }
                    }
                    Ok(Some(if vals.is_empty() { false_expr() } else { Expr::expr(col.clone()).is_in(vals) }))
                }
                _ => Err("unsupported lookup".into()),
            }
        }
        Lookup::Year | Lookup::Month | Lookup::Day => {
            let Some(v) = parse_number(raw).map_err(String::from)? else { return Ok(None) };
            let part = match d.lookup {
                Lookup::Year => "YEAR",
                Lookup::Month => "MONTH",
                _ => "DAY",
            };
            Ok(Some(match v {
                IntValue::Value(x) if d.lookup != Lookup::Year || (1..=9999).contains(&x) => {
                    Expr::expr(extract(part, col, d.kind, ctx.tz)).eq(x)
                }
                _ => false_expr(),
            }))
        }
        Lookup::DateGt | Lookup::DateGte | Lookup::DateLt | Lookup::DateLte => {
            let Some(date) = parse_date_input(raw, ctx.language)? else { return Ok(None) };
            let e = Expr::expr(local_date(col, ctx.tz));
            Ok(Some(match d.lookup {
                Lookup::DateGt => e.gt(date),
                Lookup::DateGte => e.gte(date),
                Lookup::DateLt => e.lt(date),
                _ => e.lte(date),
            }))
        }
        _ => match d.kind {
            FieldKind::Integer { min, max } => {
                let Some(v) = parse_number(raw).map_err(String::from)? else { return Ok(None) };
                Ok(Some(int_condition(col, d.lookup, v, min, max)))
            }
            FieldKind::Char => {
                let Some(v) = parse_char(raw).map_err(String::from)? else { return Ok(None) };
                Ok(char_condition(col, d.lookup, &v))
            }
            FieldKind::Date => {
                let Some(date) = parse_date_input(raw, ctx.language)? else { return Ok(None) };
                Ok(Some(cmp(Expr::expr(col.clone()), d.lookup, date)))
            }
            FieldKind::DateTime => {
                let Some(ts) = parse_iso_datetime_input(raw, ctx.tz)? else { return Ok(None) };
                Ok(Some(cmp(Expr::expr(col.clone()), d.lookup, ts)))
            }
            FieldKind::Boolean => Ok(None),
        },
    }
}

fn cmp<V: Into<sea_query::Value>>(e: Expr, lookup: Lookup, v: V) -> SimpleExpr {
    match lookup {
        Lookup::Gt => e.gt(v),
        Lookup::Gte => e.gte(v),
        Lookup::Lt => e.lt(v),
        Lookup::Lte => e.lte(v),
        _ => e.eq(v),
    }
}

// =============================================================================================
// OrderingFilter
// =============================================================================================

/// A sort term (`name` or `-name`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderTerm {
    pub field: String,
    pub descending: bool,
}

/// DRF `OrderingFilter.get_ordering`: `?ordering=a,-b` (last value), whitespace trimmed per
/// term, invalid fields **ignored**. `None` → no valid terms (the caller uses
/// the default ordering of the view or model).
pub fn parse_ordering(q: &QueryParams, valid_fields: &[&str]) -> Option<Vec<OrderTerm>> {
    let p = q.get("ordering").filter(|p| !p.is_empty())?;
    let terms: Vec<OrderTerm> = p
        .split(',')
        .map(str::trim)
        .filter_map(|t| {
            let (desc, name) = match t.strip_prefix('-') {
                Some(n) => (true, n),
                None => (false, t),
            };
            valid_fields.contains(&name).then(|| OrderTerm { field: name.to_string(), descending: desc })
        })
        .collect();
    if terms.is_empty() { None } else { Some(terms) }
}

/// Applies sort terms. Duplicate fields are dropped (Django's compiler removes duplicate
/// ORDER BY expressions; the first wins). `resolve` returns the SQL expression per field name.
pub fn apply_order_terms(select: &mut SelectStatement, terms: &[OrderTerm], resolve: impl Fn(&str) -> Option<SimpleExpr>) {
    let mut seen: Vec<&str> = Vec::new();
    for t in terms {
        if seen.contains(&t.field.as_str()) {
            continue;
        }
        seen.push(&t.field);
        if let Some(e) = resolve(&t.field) {
            select.order_by_expr(e, if t.descending { Order::Desc } else { Order::Asc });
        }
    }
}

/// Convenient: `ordering_fields` as a (name, expression) list; without a valid specification `default`
/// applies (terms like `"-created"`, also resolved via `fields`). Returns the applied terms.
pub fn apply_ordering(select: &mut SelectStatement, q: &QueryParams, fields: &[(&str, SimpleExpr)], default: &[&str]) -> Vec<OrderTerm> {
    let names: Vec<&str> = fields.iter().map(|(n, _)| *n).collect();
    let terms = parse_ordering(q, &names).unwrap_or_else(|| {
        default
            .iter()
            .map(|d| match d.strip_prefix('-') {
                Some(n) => OrderTerm { field: n.to_string(), descending: true },
                None => OrderTerm { field: d.to_string(), descending: false },
            })
            .collect()
    });
    apply_order_terms(select, &terms, |n| fields.iter().find(|(f, _)| *f == n).map(|(_, e)| e.clone()));
    terms
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_query::{PostgresQueryBuilder, Query};

    fn ctx() -> FilterContext<'static> {
        FilterContext { tz: &chrono_tz::Europe::Berlin, language: "en-us" }
    }

    fn sql(fs: &FilterSet, q: &str) -> Result<String, ApiError> {
        let mut s = Query::select();
        s.column(Alias::new("id")).from(Alias::new("t"));
        fs.apply(&mut s, &QueryParams::parse(q), &ctx())?;
        Ok(s.to_string(PostgresQueryBuilder))
    }

    fn tags() -> FilterSet {
        FilterSet::new()
            .field("id", Expr::col(Alias::new("id")), FieldKind::INT, ID_KWARGS)
            .field("name", Expr::col(Alias::new("name")), FieldKind::Char, CHAR_KWARGS)
            .declared("is_root", Expr::col(Alias::new("tn_parent_id")), FieldKind::INT, Lookup::IsNull)
    }

    #[test]
    fn numbers() {
        assert_eq!(parse_number("12"), Ok(Some(IntValue::Value(12))));
        assert_eq!(parse_number(" 1.9 "), Ok(Some(IntValue::Value(1))));
        assert_eq!(parse_number("-1.9"), Ok(Some(IntValue::Value(-1))));
        assert_eq!(parse_number("1e3"), Ok(Some(IntValue::Value(1000))));
        assert_eq!(parse_number("1_000"), Ok(Some(IntValue::Value(1000))));
        assert_eq!(parse_number(".5"), Ok(Some(IntValue::Value(0))));
        assert_eq!(parse_number("0e999999"), Ok(Some(IntValue::Value(0))));
        assert_eq!(parse_number("1e100"), Ok(Some(IntValue::Overflow { negative: false })));
        assert_eq!(parse_number(""), Ok(None));
        for bad in ["abc", " ", "NaN", "inf", "1..2", "1e", "_1", "1,5"] {
            assert_eq!(parse_number(bad), Err(MSG_ENTER_NUMBER), "{bad}");
        }
    }

    #[test]
    fn id_and_in() {
        assert_eq!(sql(&tags(), "id=5").unwrap(), r#"SELECT "id" FROM "t" WHERE "id" = 5"#);
        assert_eq!(sql(&tags(), "id__in=1,%202,,3").unwrap(), r#"SELECT "id" FROM "t" WHERE "id" IN (1, 2, 3)"#);
        assert_eq!(sql(&tags(), "id__in=,").unwrap(), r#"SELECT "id" FROM "t" WHERE 1 = 0"#);
        assert_eq!(sql(&tags(), "id__in=").unwrap(), r#"SELECT "id" FROM "t""#);
        assert_eq!(sql(&tags(), "id=99999999999").unwrap(), r#"SELECT "id" FROM "t" WHERE 1 = 0"#);
        let e = sql(&tags(), "id=x&id__in=1,a&name__icontains=%00").unwrap_err();
        assert_eq!(
            e.body_json().unwrap().to_string(),
            r#"{"id__in":["Enter a number."],"id":["Enter a number."],"name__icontains":["Null characters are not allowed."]}"#
        );
    }

    #[test]
    fn char_lookups() {
        // Bound parameter contains Django's escaping `\%` / `\_`.
        let mut s = Query::select();
        s.column(Alias::new("id")).from(Alias::new("t"));
        tags().apply(&mut s, &QueryParams::parse("name__icontains=50%25_a"), &ctx()).unwrap();
        let (q, v) = s.build(PostgresQueryBuilder);
        assert_eq!(q, r#"SELECT "id" FROM "t" WHERE UPPER(CAST("name" AS text)) LIKE UPPER($1)"#);
        assert_eq!(v.0[0], sea_query::Value::String(Some(Box::new(r"%50\%\_a%".to_string()))));
        assert_eq!(sql(&tags(), "name__iexact=Foo").unwrap(), r#"SELECT "id" FROM "t" WHERE UPPER(CAST("name" AS text)) = UPPER('Foo')"#);
        assert_eq!(
            sql(&tags(), "name__istartswith=a&name__iendswith=b").unwrap(),
            r#"SELECT "id" FROM "t" WHERE UPPER(CAST("name" AS text)) LIKE UPPER('a%') AND UPPER(CAST("name" AS text)) LIKE UPPER('%b')"#
        );
        assert_eq!(sql(&tags(), "name__icontains=%20%20").unwrap(), r#"SELECT "id" FROM "t""#);
    }

    #[test]
    fn booleans() {
        assert_eq!(sql(&tags(), "is_root=TRUE").unwrap(), r#"SELECT "id" FROM "t" WHERE "tn_parent_id" IS NULL"#);
        assert_eq!(sql(&tags(), "is_root=0").unwrap(), r#"SELECT "id" FROM "t" WHERE "tn_parent_id" IS NOT NULL"#);
        assert_eq!(sql(&tags(), "is_root=maybe").unwrap(), r#"SELECT "id" FROM "t""#);
    }

    #[test]
    fn dates() {
        let fs = FilterSet::new().field("created", Expr::col(Alias::new("created")), FieldKind::Date, DATE_KWARGS).field(
            "added",
            Expr::col(Alias::new("added")),
            FieldKind::DateTime,
            DATETIME_KWARGS,
        );
        assert_eq!(
            sql(&fs, "created__gte=2024-01-02&added__date__lt=2024-03-01&added__year=2024").unwrap(),
            r#"SELECT "id" FROM "t" WHERE "created" >= '2024-01-02' AND (EXTRACT(YEAR FROM "added" AT TIME ZONE 'Europe/Berlin')) = 2024 AND (("added" AT TIME ZONE 'Europe/Berlin')::date) < '2024-03-01'"#
        );
        assert!(sql(&fs, "added__gt=2024-01-01T00:00:00Z").unwrap().contains(r#""added" > '2024-01-01 00:00:00.000000 +00:00'"#));
        let e = sql(&fs, "created__gt=01.02.2024&added__lt=yesterday").unwrap_err();
        assert_eq!(
            e.body_json().unwrap().to_string(),
            r#"{"created__gt":["Enter a valid date."],"added__lt":["Enter a valid date/time."]}"#
        );
    }

    #[test]
    fn ordering() {
        let q = QueryParams::parse("ordering=-name,%20bogus,name,document_count");
        assert_eq!(
            parse_ordering(&q, &["name", "document_count"]).unwrap(),
            vec![
                OrderTerm { field: "name".into(), descending: true },
                OrderTerm { field: "name".into(), descending: false },
                OrderTerm { field: "document_count".into(), descending: false }
            ]
        );
        assert!(parse_ordering(&QueryParams::parse("ordering=bogus"), &["name"]).is_none());
        let mut s = Query::select();
        s.column(Alias::new("id")).from(Alias::new("t"));
        let fields = [("name", Expr::col(Alias::new("name")).into()), ("document_count", Expr::col(Alias::new("document_count")).into())];
        apply_ordering(&mut s, &q, &fields, &["name"]);
        assert_eq!(s.to_string(PostgresQueryBuilder), r#"SELECT "id" FROM "t" ORDER BY "name" DESC, "document_count" ASC"#);
        let mut s = Query::select();
        s.column(Alias::new("id")).from(Alias::new("t"));
        apply_ordering(&mut s, &QueryParams::parse("ordering=x"), &fields, &["-name"]);
        assert_eq!(s.to_string(PostgresQueryBuilder), r#"SELECT "id" FROM "t" ORDER BY "name" DESC"#);
    }
}
